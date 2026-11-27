//! Inline expansion of small fns (with `-O`).
//!
//! minclif has no IR and turns instructions straight into machine code. So expansion is done at the syntax tree stage: once the call's arguments have been built up to their ABI form
//! (the copied address for an aggregate passed by value, the destination to write for an sret return value), instead of calling, the callee's body is
//! code-generated into the same FunctionBuilder. The body's parameters are bound to those values instead of func_params, and `return` / the trailing value
//! become a jump to the continuation block (emit_return looks at this). The callee's type inference, variables and drop state are swapped in and then restored.
//!
//! What is expanded is small fns: those that contain no closures or macros, whose body syntax tree is small, and that are not special bodies (drop glue etc.) or
//! panics that report the caller's location (unwrap etc.). A caller that expanded a user fn puts the hash and line of the callee's item into its cache key
//! (session::fn_key). Fns of dependency crates are not expanded (their bodies are not in the interface hash).
//! Inside an expansion, expansion goes one more level deep

use super::*;

/// The bodies of the fns that may be expanded (fn ID → syntax tree)
#[derive(Default)]
pub struct Inline {
    pub bodies: crate::fxhash::FxMap<u32, ItemFn>,
    /// For user fns, the item's start line - 1 (the base for panic locations). Prelude fns have none
    pub user_lines: crate::fxhash::FxMap<u32, u32>,
    /// The type inference result per callee (instance) (independent of the caller, so done only once)
    pub inferred: Mutex<crate::fxhash::FxMap<u32, Inferred>>,
    /// Check only (check / LSP): the instruction sequence is built but not turned into machine code (an empty Blob is returned)
    pub check_only: bool,
}


type Inferred = (crate::ast_infer::LetTypes, crate::ast_infer::MacroArgs, crate::ast_infer::TextTypes);

/// The upper limit on the size of the body syntax tree (the length of its Debug string; the size is usually checked by session as the number of source tokens)
const MAX_BODY: usize = 6000;
/// The upper limit on the number of tokens in an item's source
pub const MAX_TOKENS: usize = 110;
/// The upper limit on the number of expansions in one fn
const MAX_PER_FN: u32 = 400;

/// Whether it has a form that may be expanded (decided from the body syntax tree alone)
pub fn inline_candidate(f: &ItemFn) -> bool {
    if f.attrs.iter().any(|a| a.contains("inline(never)") || a.contains("track_caller") || a.contains("rustc_inherit_overflow_checks")) {
        return false;
    }
    let d = format!("{:?}", f.block);
    d.len() <= MAX_BODY && !d.contains("Closure(") && !d.contains("Macro(") && !d.contains("Item(")
}

impl<'p> Cx<'p> {
    /// Expands target instead of calling it with args (the arguments in ABI form). None if not expanded.
    /// If expanded, Some(return value) (for sret, a None return value: the caller holds the destination to write)
    pub(crate) fn try_inline(&mut self, target: u32, sig: &FnSig, args: &[Value]) -> R<Option<Option<Value>>> {
        if !self.prog.opt || self.inline_depth >= 2 || self.inlined >= MAX_PER_FN || sig.ret == Ty::Never {
            return Ok(None);
        }
        let (body_id, env) = {
            let g = self.insts.lock().unwrap();
            (g.body_of(target), g.env(self.prog, target))
        };
        let Some(f) = self.inl.bodies.get(&body_id) else { return Ok(None) };
        // (a fn does not expand itself)
        if body_id == self.root {
            return Ok(None);
        }
        // A closure body: the captures (name, variable type, by-ref?, offset) are fields of `__env` (same as compile)
        let mut caps: Vec<(String, Ty, bool, u32)> = vec![];
        let (root, base_k) = match self.prog.closures.get(&body_id) {
            Some(cd) => {
                if let Some(Ty::Ptr(_, Ty::Struct(sid))) = sig.params.first() {
                    let sd = &self.prog.structs[*sid as usize];
                    for (i, (n, (_, ft, off))) in cd.captures.iter().zip(&sd.fields).enumerate() {
                        match (cd.moved.get(i).copied().unwrap_or(false), ft) {
                            (false, Ty::Ptr(_, t)) => caps.push((n.clone(), **t, true, *off)),
                            _ => caps.push((n.clone(), *ft, false, *off)),
                        }
                    }
                }
                (cd.parent, cd.k + 1)
            }
            None => (body_id, 0),
        };
        if root == self.root && base_k > 0 {
            return Ok(None);
        }
        let extra: Vec<(String, Ty)> = caps.iter().map(|c| (c.0.clone(), c.1)).collect();
        let cached = self.inl.inferred.lock().unwrap().get(&target).cloned();
        let (lets, margs, texts) = match cached {
            Some(x) => x,
            None => {
                let ((lets, _, _), margs, texts) =
                    crate::ast_infer::infer_ctx_margs(self.prog, f.sig.recv.is_some(), &f.sig.params, &f.block, sig, &env, &extra, Some(sig.ret), Some((root, base_k)));
                self.inl.inferred.lock().unwrap().insert(target, (lets.clone(), margs.clone(), texts.clone()));
                (lets, margs, texts)
            }
        };
        self.inlined += 1;
        // The continuation (the return value is the block's parameter)
        let cont = self.b.create_block();
        let sret = is_sret(sig);
        let ret_ty = if sret { None } else { clif_ty(sig.ret) };
        let rv = ret_ty.map(|t| self.b.append_block_param(cont, t));
        let entry = self.b.create_block();
        self.b.ins().jump(entry, &[]);
        self.b.switch_to_block(entry);
        // Switch to the callee's state
        let s_scopes = std::mem::replace(&mut self.scopes, vec![vec![]]);
        let s_loops = std::mem::take(&mut self.loops);
        let s_label = self.pending_label.take();
        let s_ret = std::mem::replace(&mut self.ret, sig.ret);
        let s_main = std::mem::replace(&mut self.is_main, false);
        let user_lb = self.inl.user_lines.get(&body_id).copied();
        let s_user = std::mem::replace(&mut self.user_code, user_lb.is_some());
        let s_ovf = std::mem::replace(&mut self.overflow_checks, self.prog.overflow_checks && user_lb.is_some());
        let lb0 = self.line_base;
        let s_lbase = std::mem::replace(&mut self.line_base, user_lb.unwrap_or(lb0));
        let s_ipb = self.index_panic_block.take();
        let s_caller = std::mem::replace(&mut self.caller_loc, false);
        let s_self = std::mem::replace(&mut self.self_sid, env.self_sid);
        let s_sret = std::mem::replace(&mut self.sret, if sret { args.first().copied() } else { None });
        let s_lets = std::mem::replace(&mut self.lets, lets);
        let s_taken = std::mem::replace(&mut self.addr_taken, addr_taken(&f.block, &self.prog.prim_mut_methods));
        let s_env = std::mem::replace(&mut self.env, env);
        let s_caps = std::mem::replace(&mut self.caps, caps);
        let s_root = std::mem::replace(&mut self.root, root);
        // (the callee's own: its parameters declared as its type parameters)
        let s_gargs = std::mem::replace(&mut self.generic_args, generic_args_of(self.prog, &f.sig.params, root));
        let s_gparams = std::mem::replace(&mut self.gen_params, gen_params_of(self.prog, f, root, root == body_id));
        let s_glets = std::mem::take(&mut self.gen_lets);
        let s_gpend = self.gen_pending.take();
        let s_cix = std::mem::take(&mut self.closure_ix);
        let s_hint = self.closure_hint.take();
        let s_seg = self.seg_self.take();
        let s_lb = std::mem::take(&mut self.loop_bounds);
        let s_pb = self.pending_bound.take();
        let s_pp = self.pending_pair.take();
        let s_checked = std::mem::replace(&mut self.checked, (None, vec![]));
        let s_hoisted = std::mem::take(&mut self.hoisted);
        let s_fmt = std::mem::replace(&mut self.in_fmt_args, 0);
        let s_owned = std::mem::take(&mut self.owned);
        let s_temps = std::mem::take(&mut self.temps);
        let s_ob = std::mem::replace(&mut self.owned_binds, 0);
        let s_scrut = self.last_scrut.take();
        let s_margs = std::mem::replace(&mut self.margs, margs);
        let s_texts = std::mem::replace(&mut self.texts, texts);
        let s_iret = std::mem::replace(&mut self.inline_ret, Some(cont));
        self.inline_depth += 1;
        let r = self.inline_body(f, sig, if sret { &args[1..] } else { args });
        self.inline_depth -= 1;
        // Restore
        self.scopes = s_scopes;
        self.loops = s_loops;
        self.pending_label = s_label;
        self.ret = s_ret;
        self.is_main = s_main;
        self.user_code = s_user;
        self.line_base = s_lbase;
        self.overflow_checks = s_ovf;
        self.index_panic_block = s_ipb;
        self.caller_loc = s_caller;
        self.self_sid = s_self;
        self.sret = s_sret;
        self.lets = s_lets;
        self.addr_taken = s_taken;
        self.env = s_env;
        self.caps = s_caps;
        self.root = s_root;
        self.generic_args = s_gargs;
        self.gen_params = s_gparams;
        self.gen_lets = s_glets;
        self.gen_pending = s_gpend;
        self.closure_ix = s_cix;
        self.closure_hint = s_hint;
        self.seg_self = s_seg;
        self.loop_bounds = s_lb;
        self.pending_bound = s_pb;
        self.pending_pair = s_pp;
        self.checked = s_checked;
        self.hoisted = s_hoisted;
        self.in_fmt_args = s_fmt;
        self.owned = s_owned;
        self.temps = s_temps;
        self.owned_binds = s_ob;
        self.last_scrut = s_scrut;
        self.margs = s_margs;
        self.texts = s_texts;
        self.inline_ret = s_iret;
        r?;
        self.b.switch_to_block(cont);
        Ok(Some(rv))
    }

    /// The callee's body (the same as body, but the parameters are the given values)
    fn inline_body(&mut self, f: &ItemFn, sig: &FnSig, args: &[Value]) -> R<()> {
        let mut params = args.iter().copied();
        let mut names: Vec<String> = vec![];
        if f.sig.recv.is_some() {
            names.push("self".to_string());
        }
        for (p, _) in &f.sig.params {
            names.push(pat_name(p)?);
        }
        for (name, &ty) in names.into_iter().zip(&sig.params) {
            if !ty.is_agg() && clif_ty(ty).is_none() {
                self.bind(name, ty, None);
                continue;
            }
            let Some(v) = params.next() else {
                return Err("internal: parameter count mismatch (inline)".into());
            };
            if ty.is_agg() {
                // (already copied by the caller: use that memory and drop it at the end of the body)
                self.scopes.last_mut().unwrap().push((name, Loc::Ptr(v), ty));
                self.own_local(Loc::Ptr(v), ty);
            } else {
                self.bind(name, ty, Some(v));
            }
        }
        // (a closure: expose the captures as fields of `__env`)
        if !self.caps.is_empty() {
            let env = match self.lookup("__env") {
                Some((l, _)) => self.loc_value(l),
                None => None,
            };
            let env = env.ok_or("closure without environment (inline)")?;
            for (n, t, by_ref, off) in self.caps.clone() {
                let a = if by_ref { self.b.ins().load(PTR, mf(), env, off as i32) } else { self.addr_plus(env, off as i32) };
                let loc = if clif_ty(t).is_none() && !t.is_agg() { Loc::Zst(self.bk.fresh_zst()) } else { Loc::Ptr(a) };
                self.scopes.last_mut().unwrap().push((n, loc, t));
            }
        }
        let (v, t) = self.block(&f.block, Some(self.ret))?;
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        self.same(SP, t, self.ret)?;
        self.drop_for_exit(0, 0);
        self.emit_return(v);
        Ok(())
    }
}
