//! The JS layer's output: readable ES module JS and a `.d.ts`.
//!
//! - records are plain objects, enums without data are string literals, enums with data `{ TAG: "V", _0: .. }`
//!   (the shape TS narrows as a discriminated union);
//! - numbers keep their Rust meaning: i32 wraps (`| 0`, `Math.imul`), u32 wraps (`>>> 0`), i64 is a BigInt
//!   (`BigInt.asIntN(64, ..)`), integer division truncates and fails on zero, `as` from f64 saturates;
//! - effects whose handlers all resume in tail position (`Info::direct`) are plain calls: `perform` is
//!   `$perform("Op", ..)`, which calls the nearest handler's arm, and `handle` is `$handleD`; for the other effects, a
//!   function that performs one is a generator (`function*`), `perform` is `yield`, a call to it is `yield*`, and
//!   `handle` runs the body under `$handle` (one-shot deep handlers);
//! - `match` is an `if` chain; a block, `match` or `if` in a value position becomes statements when it is the
//!   value of a `let` or of the function, and an immediately called function otherwise.

use crate::ast::*;
use crate::check::{Builtin, Callee, Info, Ty, bound_types};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write;

enum Dest {
    Return,
    Assign(String),
    Discard,
}

pub struct Js<'a> {
    info: &'a Info,
    out: String,
    ind: usize,
    tmp: u32,
    /// inside a generator (an effectful function, a handled body, a handler arm)
    gen_: bool,
    conts: Vec<String>,
    /// the continuations of the direct (tail-resumptive) handler being written: `k(x)` is just `x`
    dconts: Vec<String>,
    pub helpers: BTreeSet<&'static str>,
    pub jsx: bool,
    /// module → names imported from it (externs)
    pub imports: BTreeMap<String, BTreeSet<String>>,
    pub jsx_runtime: String,
    /// the instance being written: a numeric-bound parameter → its type
    subst: HashMap<String, Ty>,
    /// instances of numeric-bound functions still to write, and the ones written or queued
    queue: Vec<(String, Vec<Ty>)>,
    queued: HashSet<String>,
    /// `@wasm fn`s that go to wasm (empty with --all-js): not written here, their uses are collected
    pub wasm_fns: HashSet<String>,
    pub wasm_wanted: Vec<(String, Vec<Ty>)>,
    /// locals in scope: (source name, JS name), innermost last. Every JS name a function uses is unique in it
    /// (`x`, then `x$1` for a second `x`), so flattened blocks, shadowing and match arms never collide
    locals: Vec<(String, String)>,
    declared: HashSet<String>,
    /// the JS names the module itself declares or imports (functions, instances, externs, wasm glue, helpers)
    module_names: HashSet<String>,
    /// what cannot be written as JS (a `break` the emitter would have to move into a function)
    pub errors: Vec<(Pos, String)>,
}

/// The JS name of an instance of a numeric-bound function: `sum$f64`
pub fn inst_name(name: &str, targs: &[Ty]) -> String {
    let mut s = ident(name);
    for t in targs {
        s.push('$');
        s.push_str(&ts_mangle(t));
    }
    s
}

fn ts_mangle(t: &Ty) -> String {
    match t {
        Ty::I32 => "i32".into(),
        Ty::U32 => "u32".into(),
        Ty::I64 => "i64".into(),
        Ty::F32 => "f32".into(),
        Ty::F64 => "f64".into(),
        Ty::Param(p) => p.clone(),
        o => ts(o).replace(|c: char| !c.is_alphanumeric(), "_"),
    }
}

fn subst_ty(t: &Ty, m: &HashMap<String, Ty>) -> Ty {
    match t {
        Ty::Param(p) => m.get(p).cloned().unwrap_or(t.clone()),
        Ty::Array(a) => Ty::Array(Box::new(subst_ty(a, m))),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| subst_ty(t, m)).collect()),
        Ty::Fn(a, r) => Ty::Fn(a.iter().map(|t| subst_ty(t, m)).collect(), Box::new(subst_ty(r, m))),
        Ty::Named(n, a) => Ty::Named(n.clone(), a.iter().map(|t| subst_ty(t, m)).collect()),
        o => o.clone(),
    }
}

/// Every combination of the numeric types the bounds admit (a public numeric-bound function is written for each)
pub fn all_instances(generics: &[String], bounds: &[(String, String)]) -> Vec<Vec<Ty>> {
    let mut out = vec![vec![]];
    for g in generics {
        let choices = match bounds.iter().find(|b| b.0 == *g) {
            Some((_, b)) => bound_types(b),
            None => vec![Ty::Param(g.clone())],
        };
        out = out.into_iter().flat_map(|p| choices.iter().map(move |c| [p.clone(), vec![c.clone()]].concat())).collect();
    }
    out
}

const RESERVED: &[&str] = &[
    "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends",
    "finally", "for", "function", "import", "in", "instanceof", "new", "null", "return", "super", "switch", "this", "throw",
    "try", "typeof", "var", "void", "while", "with", "yield", "await", "enum", "implements", "interface", "package",
    "private", "protected", "public", "static", "arguments", "eval", "undefined", "NaN", "Infinity",
];

pub fn ident(n: &str) -> String {
    if RESERVED.contains(&n) { format!("{n}$") } else { n.to_string() }
}

fn js_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

const HELPERS: &[(&str, &str)] = &[
    ("$div_i32", "function $div_i32(a, b) {\n  if (b === 0) throw new RangeError(\"attempt to divide by zero\");\n  return (a / b) | 0;\n}"),
    ("$rem_i32", "function $rem_i32(a, b) {\n  if (b === 0) throw new RangeError(\"attempt to calculate the remainder with a divisor of zero\");\n  return (a % b) | 0;\n}"),
    ("$div_u32", "function $div_u32(a, b) {\n  if (b === 0) throw new RangeError(\"attempt to divide by zero\");\n  return (a / b) >>> 0;\n}"),
    ("$rem_u32", "function $rem_u32(a, b) {\n  if (b === 0) throw new RangeError(\"attempt to calculate the remainder with a divisor of zero\");\n  return (a % b) >>> 0;\n}"),
    ("$f64_to_i32", "function $f64_to_i32(x) {\n  return x !== x ? 0 : x >= 2147483647 ? 2147483647 : x <= -2147483648 ? -2147483648 : x | 0;\n}"),
    ("$f64_to_u32", "function $f64_to_u32(x) {\n  return x !== x || x <= 0 ? 0 : x >= 4294967295 ? 4294967295 : x >>> 0;\n}"),
    ("$f64_to_i64", "function $f64_to_i64(x) {\n  if (x !== x) return 0n;\n  if (x >= 9223372036854775807) return 9223372036854775807n;\n  if (x <= -9223372036854775808) return -9223372036854775808n;\n  return BigInt(Math.trunc(x));\n}"),
    ("$at", "function $at(a, i) {\n  if (i >= 0 && i < a.length) return a[i];\n  throw new RangeError(\"index out of bounds: the len is \" + a.length + \" but the index is \" + i);\n}"),
    ("$range", "function $range(a, b) {\n  const r = [];\n  for (let i = a; i < b; i++) r.push(i);\n  return r;\n}"),
    ("$fold", "function $fold(xs, acc, f) {\n  for (let i = 0; i < xs.length; i++) acc = f(acc, xs[i]);\n  return acc;\n}"),
    ("$rangeFold", "function $rangeFold(a, b, acc, f) {\n  for (let i = a; i < b; i++) acc = f(acc, i);\n  return acc;\n}"),
    ("$rangeMap", "function $rangeMap(a, b, f) {\n  const r = new Array(Math.max(b - a, 0));\n  for (let i = a; i < b; i++) r[i - a] = f(i);\n  return r;\n}"),
    ("$rangeMapTo", "function $rangeMapTo(T, a, b, f) {\n  const r = new T(Math.max(b - a, 0));\n  for (let i = a; i < b; i++) r[i - a] = f(i);\n  return r;\n}"),
    ("$arity1", "function $arity1(f) {\n  return (a) => f(a);\n}"),
    ("$arity2", "function $arity2(f) {\n  return (a, b) => f(a, b);\n}"),
    ("$round", "function $round(x) {\n  return x < 0 ? -Math.round(-x) : Math.round(x);\n}"),
    (
        "$handle",
        "// A deep handler: runs the generator `body`; an operation in `ops` goes to `arm` with a one-shot continuation,\n// any other operation is passed on to the enclosing handler\nfunction* $handle(body, ops, arm) {\n  const g = body();\n  function* resume(v) {\n    for (;;) {\n      const r = g.next(v);\n      if (r.done) return r.value;\n      if (ops.includes(r.value.TAG)) {\n        let used = false;\n        const k = function* (x) {\n          if (used) throw new Error(\"zustscript: a continuation was resumed twice\");\n          used = true;\n          return yield* resume(x);\n        };\n        return yield* arm(r.value, k);\n      }\n      v = yield r.value;\n    }\n  }\n  return yield* resume(undefined);\n}",
    ),
    (
        "$perform",
        "// Direct (tail-resumptive) handlers: a chain of frames; an operation calls the nearest frame's arm, which runs\n// under the handlers outside its own `handle` (deep handler semantics)\nlet $cur = null;\nfunction $perform(op, ...a) {\n  for (let f = $cur; f !== null; f = f.p) {\n    const h = f.h[op];\n    if (h !== undefined) {\n      const s = $cur;\n      $cur = f.p;\n      try {\n        return h(...a);\n      } finally {\n        $cur = s;\n      }\n    }\n  }\n  throw new Error(\"zustscript: unhandled effect \" + op);\n}\n\nfunction $handleD(h, body) {\n  const f = { h, p: $cur };\n  $cur = f;\n  try {\n    return body();\n  } finally {\n    $cur = f.p;\n  }\n}",
    ),
    ("$run", "function $run(g) {\n  const r = g.next();\n  if (!r.done) throw new Error(\"zustscript: unhandled effect \" + r.value.TAG);\n  return r.value;\n}"),
];

impl<'a> Js<'a> {
    pub fn new(info: &'a Info, jsx_runtime: &str) -> Js<'a> {
        Js {
            info,
            out: String::new(),
            ind: 0,
            tmp: 0,
            gen_: false,
            conts: vec![],
            dconts: vec![],
            helpers: BTreeSet::new(),
            jsx: false,
            imports: BTreeMap::new(),
            jsx_runtime: jsx_runtime.to_string(),
            subst: HashMap::new(),
            queue: vec![],
            queued: HashSet::new(),
            wasm_fns: HashSet::new(),
            wasm_wanted: vec![],
            locals: vec![],
            declared: HashSet::new(),
            module_names: HashSet::new(),
            errors: vec![],
        }
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.ind {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn fresh(&mut self) -> String {
        self.tmp += 1;
        format!("$t{}", self.tmp)
    }

    fn string_enum(&self, en: &str) -> bool {
        let e = &self.info.enums[en];
        e.unit_only() && !e.is_effect()
    }

    /// The body of the module: the functions of the JS layer
    pub fn items(&mut self, items: &[Item]) -> String {
        for it in items {
            match it {
                Item::Fn { name, .. } | Item::WasmFn { name, .. } => {
                    self.module_names.insert(ident(name));
                }
                Item::Extern { name, js, .. } => {
                    self.module_names.insert(ident(name));
                    self.module_names.insert(js.split('.').next().unwrap_or(js).to_string());
                }
                _ => {}
            }
        }
        for t in ["Math", "String", "Number", "BigInt", "Array", "Object", "Float64Array", "Float32Array", "Int32Array", "console"] {
            self.module_names.insert(t.to_string());
        }
        let mut bodies: HashMap<&String, &Item> = HashMap::new();
        for it in items {
            if let Item::Fn { name, public, generics, bounds, .. } = it {
                if self.wasm_fns.contains(name) {
                    if *public {
                        for targs in all_instances(generics, bounds) {
                            self.want(name, targs);
                        }
                    }
                    continue;
                }
                if bounds.is_empty() {
                    self.function(it, &ident(name), *public);
                } else {
                    bodies.insert(name, it);
                    if *public {
                        for targs in all_instances(generics, bounds) {
                            self.want(name, targs);
                        }
                    }
                }
            }
        }
        // numeric-bound functions: one JS function per instance used (the arithmetic differs per type)
        while let Some((name, targs)) = self.queue.pop() {
            let it = bodies[&name];
            let Item::Fn { generics, public, .. } = it else { unreachable!() };
            self.subst = generics.iter().cloned().zip(targs.iter().cloned()).collect();
            self.function(it, &inst_name(&name, &targs), *public);
            self.subst.clear();
        }
        std::mem::take(&mut self.out)
    }

    fn want(&mut self, name: &str, targs: Vec<Ty>) -> String {
        let n = inst_name(name, &targs);
        if self.queued.insert(n.clone()) {
            if self.wasm_fns.contains(name) {
                // (its glue, named the same, is written with the wasm layer)
                self.wasm_wanted.push((name.to_string(), targs));
            } else {
                self.queue.push((name.to_string(), targs));
            }
        }
        n
    }

    /// A JS name for a new local `n`, unique in the function (not in scope yet: see `push_local`)
    fn reserve(&mut self, n: &str) -> String {
        let base = ident(n);
        let mut js = base.clone();
        let mut k = 1;
        while self.declared.contains(&js) || self.module_names.contains(&js) {
            js = format!("{base}${k}");
            k += 1;
        }
        self.declared.insert(js.clone());
        js
    }

    fn push_local(&mut self, n: &str, js: &str) {
        self.locals.push((n.to_string(), js.to_string()));
    }

    /// Declares local `n` (in scope from here)
    fn bind(&mut self, n: &str) -> String {
        let js = self.reserve(n);
        self.push_local(n, &js);
        js
    }

    /// The JS name of local `n`
    fn local(&self, n: &str) -> String {
        self.locals.iter().rev().find(|x| x.0 == n).map_or_else(|| ident(n), |x| x.1.clone())
    }

    fn function(&mut self, it: &Item, js_name: &str, public: bool) {
        let Item::Fn { name, args, body, .. } = it else { return };
        let gen_ = self.generator(name);
        self.locals.clear();
        // (the module's own names: a local never takes one, so a call of a top-level function still reaches it)
        self.declared.clear();
        let ps: Vec<String> = args.iter().map(|a| self.bind(&a.0)).collect();
        let saved = std::mem::take(&mut self.out);
        self.line(&format!("{}function{} {js_name}({}) {{", if public { "export " } else { "" }, if gen_ { "*" } else { "" }, ps.join(", ")));
        self.ind += 1;
        self.gen_ = gen_;
        self.tmp = 0;
        self.stmt_expr(body, &Dest::Return);
        self.ind -= 1;
        self.line("}");
        self.out.push('\n');
        let f = std::mem::replace(&mut self.out, saved);
        self.out.push_str(&f);
    }

    /// The name of a use of function `n` (an instance's, for a numeric-bound one)
    fn fn_name(&mut self, id: Id, n: &str) -> String {
        if self.wasm_fns.contains(n) && !self.info.inst.contains_key(&id) {
            return self.want(n, vec![]);
        }
        match self.info.inst.get(&id).cloned() {
            Some(targs) => {
                let targs: Vec<Ty> = targs.iter().map(|t| subst_ty(t, &self.subst)).collect();
                self.want(n, targs)
            }
            None => ident(n),
        }
    }

    /// The module's header: imports, then the runtime helpers used
    pub fn header(&self) -> String {
        let mut h = String::new();
        if self.jsx {
            let _ = writeln!(h, "import {{ jsx as _jsx, jsxs as _jsxs, Fragment as _Fragment }} from {};", js_str(&self.jsx_runtime));
        }
        for (m, names) in &self.imports {
            let _ = writeln!(h, "import {{ {} }} from {};", names.iter().cloned().collect::<Vec<_>>().join(", "), js_str(m));
        }
        if !h.is_empty() {
            h.push('\n');
        }
        let mut need: BTreeSet<&str> = self.helpers.clone();
        if need.contains("$handle") {
            need.insert("$run");
        }
        for (n, code) in HELPERS {
            if need.contains(n) {
                h.push_str(code);
                h.push_str("\n\n");
            }
        }
        h
    }

    // ------------------------------------------------------------ statements

    fn assign(&mut self, d: &Dest, v: String) {
        // (`(yield x)` needs no parentheses as a whole statement)
        let v = if (v.starts_with("(yield ") || v.starts_with("(yield* ")) && v.ends_with(')') && balanced(&v[1..v.len() - 1]) && !matches!(d, Dest::Assign(_)) {
            v[1..v.len() - 1].to_string()
        } else {
            v
        };
        match d {
            Dest::Return => self.line(&format!("return {v};")),
            Dest::Assign(t) => self.line(&format!("{t} = {v};")),
            Dest::Discard => self.line(&format!("{v};")),
        }
    }

    /// Needs statements to be written as JS (otherwise it is an expression)
    fn complex(e: &Expr) -> bool {
        match &e.kind {
            EK::Block(s, t) => !s.is_empty() || t.as_ref().is_some_and(|t| Self::complex(t)),
            EK::Match(..) | EK::Assign(..) | EK::While(..) | EK::For(..) | EK::Break | EK::Continue => true,
            EK::If(_, t, el) => Self::complex(t) || el.as_ref().is_none_or(|x| Self::complex(x)),
            _ => false,
        }
    }

    /// A loop's body: its statements, the value dropped
    fn loop_body(&mut self, body: &Expr) {
        self.ind += 1;
        self.stmt_expr(body, &Dest::Discard);
        self.ind -= 1;
        self.line("}");
    }

    fn stmt_expr(&mut self, e: &Expr, d: &Dest) {
        // (the statements that have no value: `()` for a destination that wants one)
        let unit = |js: &mut Js| {
            if let Dest::Assign(t) = d {
                js.line(&format!("{t} = undefined;"));
            }
        };
        match &e.kind {
            EK::Assign(n, x) => {
                let target = self.local(n);
                if Self::complex(x) {
                    self.stmt_expr(x, &Dest::Assign(target));
                } else {
                    let v = self.expr(x);
                    self.line(&format!("{target} = {v};"));
                }
                unit(self);
                return;
            }
            EK::While(c, body) => {
                let c = self.expr(c);
                self.line(&format!("while ({c}) {{"));
                self.loop_body(body);
                unit(self);
                return;
            }
            EK::For(x, it, body) => {
                let range = match &it.kind {
                    EK::Call(f, args) if matches!(self.info.callee.get(&f.id), Some(Callee::Range)) => Some(args),
                    _ => None,
                };
                let mark = self.locals.len();
                match range {
                    Some(args) => {
                        // `for i in range(a, b)`: a counting loop. Both bounds are evaluated once, in order, before
                        // the loop (the body may assign a variable the end was read from)
                        let lit = |e: &Expr| matches!(e.kind, EK::Lit(Lit::Int(_)));
                        let mut a = self.expr(&args[0]);
                        if !lit(&args[0]) {
                            let t = self.fresh();
                            self.line(&format!("const {t} = {a};"));
                            a = t;
                        }
                        let mut b = self.expr(&args[1]);
                        if !lit(&args[1]) {
                            let t = self.fresh();
                            self.line(&format!("const {t} = {b};"));
                            b = t;
                        }
                        let x = self.bind(x);
                        self.line(&format!("for (let {x} = {a}; {x} < {b}; {x}++) {{"));
                    }
                    None => {
                        let v = self.expr(it);
                        let x = self.bind(x);
                        self.line(&format!("for (const {x} of {v}) {{"));
                    }
                }
                self.loop_body(body);
                self.locals.truncate(mark);
                unit(self);
                return;
            }
            EK::Break => return self.line("break;"),
            EK::Continue => return self.line("continue;"),
            _ => {}
        }
        match &e.kind {
            EK::Block(stmts, tail) => {
                let mark = self.locals.len();
                for s in stmts {
                    match s {
                        Stmt::Let(p, _, x) => self.let_(p, x),
                        Stmt::LetMut(n, _, x) => {
                            // (the initializer sees the names before this one)
                            if Self::complex(x) {
                                let js = self.reserve(n);
                                self.line(&format!("let {js};"));
                                self.stmt_expr(x, &Dest::Assign(js.clone()));
                                self.push_local(n, &js);
                            } else {
                                let v = self.expr(x);
                                let js = self.bind(n);
                                self.line(&format!("let {js} = {v};"));
                            }
                        }
                        Stmt::Expr(x) => self.stmt_expr(x, &Dest::Discard),
                    }
                }
                match tail {
                    Some(t) => self.stmt_expr(t, d),
                    None => {
                        if let Dest::Assign(t) = d {
                            self.line(&format!("{t} = undefined;"));
                        }
                    }
                }
                self.locals.truncate(mark);
            }
            EK::If(c, t, el) if Self::complex(e) || matches!(d, Dest::Discard) => {
                let c = self.expr(c);
                self.line(&format!("if ({c}) {{"));
                self.ind += 1;
                self.stmt_expr(t, d);
                self.ind -= 1;
                let mut el = el.as_deref();
                while let Some(x) = el {
                    if let EK::If(c2, t2, el2) = &x.kind {
                        let c2 = self.expr(c2);
                        self.line(&format!("}} else if ({c2}) {{"));
                        self.ind += 1;
                        self.stmt_expr(t2, d);
                        self.ind -= 1;
                        el = el2.as_deref();
                    } else {
                        self.line("} else {");
                        self.ind += 1;
                        self.stmt_expr(x, d);
                        self.ind -= 1;
                        el = None;
                    }
                }
                self.line("}");
            }
            EK::Match(s, arms) => {
                // (an arm's bindings get names of their own, so they never hide the subject)
                let subj = match &s.kind {
                    EK::Var(n) if matches!(self.info.callee.get(&s.id), Some(Callee::Local)) => self.local(n),
                    _ => {
                        let t = self.fresh();
                        let v = self.expr(s);
                        self.line(&format!("const {t} = {v};"));
                        t
                    }
                };
                let mut opened = false;
                for (k, (p, body)) in arms.iter().enumerate() {
                    let last = k + 1 == arms.len();
                    let conds = self.pat_cond(p, &subj);
                    if k == 0 && !(conds.is_empty() || last) {
                        opened = true;
                    }
                    let head = if conds.is_empty() || last {
                        if k == 0 { None } else { Some("} else {".to_string()) }
                    } else {
                        Some(format!("{}if ({}) {{", if k == 0 { "" } else { "} else " }, conds.join(" && ")))
                    };
                    let open = head.is_some();
                    if let Some(h) = head {
                        self.line(&h);
                        self.ind += 1;
                    }
                    let mark = self.locals.len();
                    let mut binds = vec![];
                    self.pat_binds(p, &subj, &mut binds);
                    for (n, v) in binds {
                        let js = self.bind(&n);
                        self.line(&format!("const {js} = {v};"));
                    }
                    self.stmt_expr(body, d);
                    self.locals.truncate(mark);
                    if open {
                        self.ind -= 1;
                    }
                    if conds.is_empty() || last {
                        break;
                    }
                }
                // (closed only if the first arm opened an `if`: a lone or irrefutable first arm opens nothing)
                if opened {
                    self.line("}");
                }
            }
            _ => {
                let v = self.expr(e);
                match d {
                    Dest::Discard if v == "undefined" => {}
                    _ => self.assign(d, v),
                }
            }
        }
    }

    fn let_(&mut self, p: &Pat, x: &Expr) {
        if Self::complex(x) {
            // (the initializer sees the names before this `let`: the new name is in scope after it)
            let t = match p {
                Pat::Bind(n) => self.reserve(n),
                _ => self.fresh(),
            };
            self.line(&format!("let {t};"));
            self.stmt_expr(x, &Dest::Assign(t.clone()));
            if let Pat::Bind(n) = p {
                self.push_local(n, &t);
            } else {
                let lhs = self.pat_lhs(p);
                self.line(&format!("const {lhs} = {t};"));
            }
            return;
        }
        let v = self.expr(x);
        match p {
            Pat::Wild => self.line(&format!("{v};")),
            _ => {
                let lhs = self.pat_lhs(p);
                self.line(&format!("const {lhs} = {v};"));
            }
        }
    }

    /// An irrefutable pattern as a JS binding target (`[a, b]`)
    fn pat_lhs(&mut self, p: &Pat) -> String {
        match p {
            Pat::Bind(n) => self.bind(n),
            Pat::Tuple(ps) => format!("[{}]", ps.iter().map(|p| if matches!(p, Pat::Wild) { String::new() } else { self.pat_lhs(p) }).collect::<Vec<_>>().join(", ")),
            _ => self.fresh(),
        }
    }

    fn pat_cond(&self, p: &Pat, s: &str) -> Vec<String> {
        match p {
            Pat::Wild | Pat::Bind(_) => vec![],
            Pat::Lit(l) => vec![format!("{s} {} {}", if matches!(l, Lit::Int(_)) { "==" } else { "===" }, lit(l, false))],
            Pat::Tuple(ps) => ps.iter().enumerate().flat_map(|(k, p)| self.pat_cond(p, &format!("{s}[{k}]"))).collect(),
            Pat::Variant(_, v, subs, _) => {
                let en = &self.info.variant_of[v];
                if self.string_enum(en) {
                    return vec![format!("{s} === {}", js_str(v))];
                }
                let mut c = vec![format!("{s}.TAG === {}", js_str(v))];
                for (k, sp) in subs.iter().enumerate() {
                    c.extend(self.pat_cond(sp, &format!("{s}._{k}")));
                }
                c
            }
        }
    }

    fn pat_binds(&self, p: &Pat, s: &str, out: &mut Vec<(String, String)>) {
        match p {
            Pat::Bind(n) => out.push((n.clone(), s.to_string())),
            Pat::Tuple(ps) => {
                for (k, p) in ps.iter().enumerate() {
                    self.pat_binds(p, &format!("{s}[{k}]"), out);
                }
            }
            Pat::Variant(_, _, subs, _) => {
                for (k, p) in subs.iter().enumerate() {
                    self.pat_binds(p, &format!("{s}._{k}"), out);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ expressions

    /// An expression as an operand of a member access, a call or a unary operator (parenthesized unless atomic)
    fn operand(&mut self, e: &Expr) -> String {
        let s = self.expr(e);
        if self.js_prec(e, &s) >= 17 { s } else { format!("({s})") }
    }

    /// An operand of a binary operator of precedence `p` (`right`: the right side, which needs parentheses at
    /// the same precedence too)
    fn bin_operand(&mut self, e: &Expr, p: u8, right: bool) -> String {
        let s = self.expr(e);
        let q = self.js_prec(e, &s);
        if q > p || (q == p && !right) { s } else { format!("({s})") }
    }

    /// The JS precedence of what `e` was written as (MDN's table: 17 member/call, 14 unary, 13 `*`, 12 `+`,
    /// 11 shifts, 10 relational, 9 equality, 6 `|`, 5 `&&`, 4 `||`, 2 `?:` and arrows)
    fn js_prec(&self, e: &Expr, s: &str) -> u8 {
        let atom = |s: &str| !s.is_empty() && !s.starts_with('-') && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.' || c == '"');
        if atom(s) || (s.starts_with('(') && s.ends_with(')') && balanced(&s[1..s.len() - 1])) {
            return 20;
        }
        match &e.kind {
            EK::Var(_) | EK::Path(..) | EK::Call(..) | EK::Method(..) | EK::Field(..) | EK::Array(_) | EK::Tuple(_) | EK::Record(..) | EK::Jsx(_) => 17,
            EK::Lit(Lit::Int(n)) if *n < 0 => 14,
            EK::Lit(_) => 20,
            EK::Un(..) if s.ends_with("| 0") => 6,
            EK::Un(..) if s.ends_with(">>> 0") => 11,
            EK::Un(..) => if single_call(s) { 17 } else { 14 },
            EK::Bin(op, ..) => {
                if s.ends_with(") | 0") {
                    6
                } else if s.ends_with(">>> 0") {
                    11
                } else if single_call(s) {
                    17
                } else {
                    bin_prec(op)
                }
            }
            EK::Cast(x, _) => {
                if s.ends_with("| 0") {
                    6
                } else if s.ends_with(">>> 0") {
                    11
                } else if single_call(s) {
                    17
                } else {
                    // (a cast that writes nothing: its operand's own precedence)
                    self.js_prec(x, s)
                }
            }
            _ => 2,
        }
    }

    /// The function is a generator: it performs (and does not handle) an effect that is not direct
    fn generator(&self, name: &str) -> bool {
        self.info.effects.get(name).is_some_and(|es| es.iter().any(|e| !self.info.direct.contains(e)))
    }

    /// A tail-resumptive handler: the arms are plain functions that `$perform` calls, `k(x)` in an arm is `x`, and
    /// the body runs as a plain function under the handler
    fn direct_handle(&mut self, body: &Expr, arms: &[HArm]) -> String {
        self.helpers.insert("$perform");
        let saved = (std::mem::take(&mut self.out), self.ind, std::mem::take(&mut self.conts), std::mem::take(&mut self.dconts), self.gen_);
        self.gen_ = false;
        let pad = "  ".repeat(saved.1);
        let mut parts = vec![];
        for a in arms {
            self.ind = saved.1 + 1;
            let mark = self.locals.len();
            let ps: Vec<String> = (0..a.pats.len()).map(|i| format!("$a{i}")).collect();
            let mut binds = vec![];
            for (i, p) in a.pats.iter().enumerate() {
                self.pat_binds(p, &format!("$a{i}"), &mut binds);
            }
            for (n, v) in binds {
                let js = self.bind(&n);
                self.line(&format!("const {js} = {v};"));
            }
            self.dconts.push(a.k.clone());
            self.stmt_expr(&a.body, &Dest::Return);
            self.dconts.pop();
            self.locals.truncate(mark);
            let b = std::mem::take(&mut self.out);
            parts.push(format!("{}: ({}) => {{
{b}{pad}}}", a.variant, ps.join(", ")));
        }
        self.ind = saved.1 + 1;
        self.dconts.clear();
        self.stmt_expr(body, &Dest::Return);
        let b = std::mem::take(&mut self.out);
        (self.out, self.ind, self.conts, self.dconts, self.gen_) = saved;
        format!("$handleD({{ {} }}, () => {{\n{b}{pad}}})", parts.join(", "))
    }

    fn iife(&mut self, e: &Expr) -> String {
        if let Some(pos) = loop_exit(e) {
            self.errors.push((pos, "`break` / `continue` here would have to leave a function in the JS output: move this `if` / `match` into a statement of its own (`let v = if ..;`)".into()));
        }
        let save = (std::mem::take(&mut self.out), self.ind);
        self.ind = 1;
        self.stmt_expr(e, &Dest::Return);
        let body = std::mem::replace(&mut self.out, save.0);
        self.ind = save.1;
        let pad = "  ".repeat(self.ind);
        let body: String = body.lines().map(|l| format!("{pad}{l}\n")).collect();
        if self.gen_ {
            format!("(yield* (function* () {{\n{body}{pad}}})())")
        } else {
            format!("(() => {{\n{body}{pad}}})()")
        }
    }

    fn num(&self, id: Id) -> Ty {
        subst_ty(&self.info.num.get(&id).cloned().unwrap_or(Ty::F64), &self.subst)
    }

    fn variant_value(&mut self, en: &str, v: &str, args: Vec<String>) -> String {
        if self.string_enum(en) {
            return js_str(v);
        }
        let mut s = format!("{{ TAG: {}", js_str(v));
        for (k, a) in args.iter().enumerate() {
            let _ = write!(s, ", _{k}: {a}");
        }
        s.push_str(" }");
        s
    }

    /// A function value passed to a builtin that calls it with extra arguments (`map` passes the index): a closure
    /// literal as is, anything else wrapped so it receives exactly its own arguments
    fn callback(&mut self, f: &Expr, arity: usize) -> String {
        let s = self.expr(f);
        if matches!(f.kind, EK::Closure(..)) {
            return s;
        }
        if !s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            // (evaluated once, before the iteration: anything but a plain name, a member access too)
            let h = if arity == 1 { "$arity1" } else { "$arity2" };
            self.helpers.insert(h);
            return format!("{h}({s})");
        }
        let ps: Vec<String> = (0..arity).map(|k| format!("$x{k}")).collect();
        format!("({}) => {}({})", ps.join(", "), s, ps.join(", "))
    }

    fn wrap_callee(&self, s: &str) -> String {
        if s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.') { s.to_string() } else { format!("({s})") }
    }

    pub fn expr(&mut self, e: &Expr) -> String {
        if Self::complex(e) {
            return self.iife(e);
        }
        match &e.kind {
            EK::Lit(Lit::Float(f)) if self.num(e.id) == Ty::F32 => f32_lit(f),
            EK::Lit(l) => lit(l, self.num(e.id) == Ty::I64),
            EK::Var(n) | EK::Path(_, n) => match self.info.callee.get(&e.id).cloned() {
                Some(Callee::Variant(en)) => {
                    let nf = self.info.enums[&en].variants.iter().find(|v| v.0 == *n).map_or(0, |v| v.1.len());
                    if nf == 0 {
                        self.variant_value(&en, n, vec![])
                    } else {
                        let ps: Vec<String> = (0..nf).map(|k| format!("a{k}")).collect();
                        let v = self.variant_value(&en, n, ps.clone());
                        format!("({}) => ({v})", ps.join(", "))
                    }
                }
                Some(Callee::Extern(js)) => {
                    self.note_extern(n);
                    js
                }
                Some(Callee::Fn) => self.fn_name(e.id, n),
                Some(Callee::Range) => {
                    self.helpers.insert("$range");
                    "$range".into()
                }
                Some(Callee::Local) => self.local(n),
                _ => ident(n),
            },
            EK::Call(f, args) => {
                if let EK::Path(_, c) = &f.kind
                    && (c == "zero" || c == "one")
                    && self.info.num.contains_key(&f.id)
                {
                    let one = c == "one";
                    return match self.num(f.id) {
                        Ty::I64 => (if one { "1n" } else { "0n" }).into(),
                        _ => (if one { "1" } else { "0" }).into(),
                    };
                }
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                if let Some(Callee::TypedCtor(el, c)) = self.info.callee.get(&f.id).cloned() {
                    let ctor = typed_js(&el);
                    return if c == "zeros" { format!("new {ctor}({})", a[0]) } else { format!("{ctor}.from({})", a[0]) };
                }
                if let EK::Var(n) | EK::Path(_, n) = &f.kind {
                    match self.info.callee.get(&f.id).cloned() {
                        Some(Callee::Variant(en)) => return self.variant_value(&en, n, a),
                        Some(Callee::Fn) if self.generator(n) => {
                            let fname = self.fn_name(f.id, n);
                            return format!("(yield* {fname}({}))", a.join(", "));
                        }
                        Some(Callee::Local) if self.dconts.contains(n) => {
                            // (tail-resumptive: the arm's value is the operation's result)
                            return if a.is_empty() { "undefined".into() } else { a[0].clone() };
                        }
                        Some(Callee::Local) if self.conts.contains(n) => {
                            let a = if a.len() == 1 && a[0] == "undefined" { vec![] } else { a };
                            return format!("(yield* {}({}))", self.local(n), a.join(", "));
                        }
                        Some(Callee::Extern(js)) => {
                            self.note_extern(n);
                            return format!("{js}({})", a.join(", "));
                        }
                        _ => {}
                    }
                }
                let f = self.expr(f);
                format!("{}({})", self.wrap_callee(&f), a.join(", "))
            }
            EK::Method(r, m, args) => {
                let b = self.info.builtin.get(&e.id).copied();
                // `range(a, b).fold(..)` / `.map(..)`: a counting loop, no array of indices
                if let EK::Call(f, ra) = &r.kind
                    && matches!(self.info.callee.get(&f.id), Some(Callee::Range))
                    && matches!(b, Some(Builtin::Fold | Builtin::Map))
                {
                    let (lo, hi) = (self.expr(&ra[0]), self.expr(&ra[1]));
                    return if b == Some(Builtin::Fold) {
                        self.helpers.insert("$rangeFold");
                        let f = self.callback(&args[1], 2);
                        let init = self.expr(&args[0]);
                        format!("$rangeFold({lo}, {hi}, {init}, {f})")
                    } else if let Some(el) = self.info.typed_lit.get(&e.id).cloned() {
                        // (into a typed array, as the type says)
                        self.helpers.insert("$rangeMapTo");
                        let f = self.callback(&args[0], 1);
                        format!("$rangeMapTo({}, {lo}, {hi}, {f})", typed_js(&el))
                    } else {
                        self.helpers.insert("$rangeMap");
                        let f = self.callback(&args[0], 1);
                        format!("$rangeMap({lo}, {hi}, {f})")
                    };
                }
                let recv = self.operand(r);
                let arg = |js: &mut Js, k: usize| js.expr(&args[k]);
                use Builtin as B;
                match b {
                    Some(B::Len) => format!("{recv}.length"),
                    Some(B::Map) => format!("{recv}.map({})", self.callback(&args[0], 1)),
                    Some(B::Filter) => format!("{recv}.filter({})", self.callback(&args[0], 1)),
                    Some(B::Any) => format!("{recv}.some({})", self.callback(&args[0], 1)),
                    Some(B::All) => format!("{recv}.every({})", self.callback(&args[0], 1)),
                    Some(B::Fold) => {
                        // (a counting loop: Array.prototype.reduce is several times slower in V8)
                        self.helpers.insert("$fold");
                        let f = self.callback(&args[1], 2);
                        format!("$fold({recv}, {}, {f})", arg(self, 0))
                    }
                    Some(B::Push) => format!("[...{recv}, {}]", arg(self, 0)),
                    Some(B::Concat) => format!("[...{recv}, ...{}]", self.operand(&args[0])),
                    Some(B::Join) => format!("{recv}.join({})", arg(self, 0)),
                    Some(B::ToUpper) => format!("{recv}.toUpperCase()"),
                    Some(B::Trim) => format!("{recv}.trim()"),
                    Some(B::Contains) => format!("{recv}.includes({})", arg(self, 0)),
                    Some(B::Split) => format!("{recv}.split({})", arg(self, 0)),
                    Some(B::ToString) => format!("String({})", self.expr(r)),
                    Some(B::Floor) => format!("Math.floor({})", self.expr(r)),
                    Some(B::Sqrt) if self.num(e.id) == Ty::F32 => format!("Math.fround(Math.sqrt({}))", self.expr(r)),
                    Some(B::Sqrt) => format!("Math.sqrt({})", self.expr(r)),
                    Some(B::Max) => format!("Math.max({}, {})", self.expr(r), arg(self, 0)),
                    Some(B::Min) => format!("Math.min({}, {})", self.expr(r), arg(self, 0)),
                    Some(B::ToArray) => format!("Array.from({recv})"),
                    Some(B::Abs) => format!("Math.abs({})", self.expr(r)),
                    Some(B::Round) => {
                        self.helpers.insert("$round");
                        format!("$round({})", self.expr(r))
                    }
                    None => {
                        let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                        format!("{recv}.{m}({})", a.join(", "))
                    }
                }
            }
            EK::Index(x, i) => {
                self.helpers.insert("$at");
                format!("$at({}, {})", self.expr(x), self.expr(i))
            }
            EK::Field(x, f) => {
                let s = self.operand(x);
                if f.parse::<usize>().is_ok() { format!("{s}[{f}]") } else { format!("{s}.{f}") }
            }
            EK::Bin(op, l, r) => self.binary(e.id, op, l, r),
            EK::Un("!", x) => format!("!{}", self.operand(x)),
            EK::Un(_, x) => {
                if let EK::Lit(Lit::Int(n)) = &x.kind {
                    return lit(&Lit::Int(-n), self.num(x.id) == Ty::I64);
                }
                if let EK::Lit(Lit::Float(f)) = &x.kind {
                    return if self.num(x.id) == Ty::F32 { format!("-{}", f32_lit(f)) } else { format!("-{f}") };
                }
                let v = self.operand(x);
                match self.num(e.id) {
                    Ty::I32 => format!("-{v} | 0"),
                    Ty::U32 => format!("-{v} >>> 0"),
                    Ty::I64 => format!("BigInt.asIntN(64, -{v})"),
                    _ => format!("-{v}"),
                }
            }
            EK::If(c, t, el) => {
                let c = self.bin_operand(c, 3, false);
                let t = self.bin_operand(t, 2, true);
                let el = el.as_ref().map(|x| self.bin_operand(x, 2, true)).unwrap_or("undefined".into());
                format!("{c} ? {t} : {el}")
            }
            EK::Block(_, Some(t)) => self.expr(t),
            EK::Block(_, None) => "undefined".into(),
            EK::Match(..) | EK::Assign(..) | EK::While(..) | EK::For(..) | EK::Break | EK::Continue => unreachable!(),
            EK::Closure(ps, body) => {
                let saved = (self.gen_, std::mem::take(&mut self.conts), std::mem::take(&mut self.dconts));
                self.gen_ = false;
                let mark = self.locals.len();
                let ps: Vec<String> = ps.iter().map(|p| self.bind(&p.0)).collect();
                let b = if Self::complex(body) {
                    let save = (std::mem::take(&mut self.out), self.ind);
                    self.ind += 1;
                    self.stmt_expr(body, &Dest::Return);
                    let inner = std::mem::replace(&mut self.out, save.0);
                    self.ind = save.1;
                    format!("{{\n{inner}{}}}", "  ".repeat(self.ind))
                } else {
                    let b = self.expr(body);
                    if b.starts_with('{') { format!("({b})") } else { b }
                };
                (self.gen_, self.conts, self.dconts) = saved;
                self.locals.truncate(mark);
                format!("({}) => {b}", ps.join(", "))
            }
            EK::Record(_, fs, base) => {
                let mut parts = vec![];
                if let Some(b) = base {
                    parts.push(format!("...{}", self.operand(b)));
                }
                for (f, x) in fs {
                    let v = self.expr(x);
                    parts.push(if v == *f { f.clone() } else { format!("{f}: {v}") });
                }
                format!("{{ {} }}", parts.join(", "))
            }
            EK::Array(xs) if self.info.typed_lit.contains_key(&e.id) => {
                let ctor = typed_js(&self.info.typed_lit[&e.id].clone());
                format!("new {ctor}([{}])", xs.iter().map(|x| self.expr(x)).collect::<Vec<_>>().join(", "))
            }
            EK::Array(xs) | EK::Tuple(xs) => format!("[{}]", xs.iter().map(|x| self.expr(x)).collect::<Vec<_>>().join(", ")),
            EK::Jsx(j) => self.jsx_expr(j),
            EK::Cast(x, _) => {
                let (from, to) = self.info.cast[&e.id].clone();
                let (from, to) = (subst_ty(&from, &self.subst), subst_ty(&to, &self.subst));
                let v = self.expr(x);
                let o = || if v.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') { v.clone() } else { format!("({v})") };
                match (&from, &to) {
                    (a, b) if a == b => v,
                    (Ty::I32 | Ty::U32 | Ty::F32, Ty::F64) => v,
                    (Ty::I32 | Ty::U32 | Ty::F64, Ty::F32) => format!("Math.fround({v})"),
                    (Ty::I64, Ty::F32) => format!("Math.fround(Number({v}))"),
                    (Ty::F32, Ty::I32) => self.help("$f64_to_i32", &v),
                    (Ty::F32, Ty::U32) => self.help("$f64_to_u32", &v),
                    (Ty::F32, Ty::I64) => self.help("$f64_to_i64", &v),
                    (Ty::I32, Ty::U32) => format!("{} >>> 0", o()),
                    (Ty::U32, Ty::I32) => format!("{} | 0", o()),
                    (Ty::I32 | Ty::U32, Ty::I64) => format!("BigInt({v})"),
                    (Ty::I64, Ty::I32) => format!("Number(BigInt.asIntN(32, {v}))"),
                    (Ty::I64, Ty::U32) => format!("Number(BigInt.asUintN(32, {v}))"),
                    (Ty::I64, Ty::F64) => format!("Number({v})"),
                    (Ty::F64, Ty::I32) => self.help("$f64_to_i32", &v),
                    (Ty::F64, Ty::U32) => self.help("$f64_to_u32", &v),
                    (Ty::F64, Ty::I64) => self.help("$f64_to_i64", &v),
                    _ => v,
                }
            }
            EK::Perform(_, v, args) => {
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                let en = self.info.variant_of[v].clone();
                if self.info.direct.contains(&en) {
                    self.helpers.insert("$perform");
                    return format!("$perform({})", [js_str(v)].into_iter().chain(a).collect::<Vec<_>>().join(", "));
                }
                format!("(yield {})", self.variant_value(&en, v, a))
            }
            EK::Handle(body, arms) if arms.iter().all(|a| self.info.variant_of.get(&a.variant).is_some_and(|en| self.info.direct.contains(en))) => {
                self.direct_handle(body, arms)
            }
            EK::Handle(body, arms) => {
                self.helpers.insert("$handle");
                let saved = (self.gen_, self.conts.clone(), std::mem::take(&mut self.out), self.ind);
                self.gen_ = true;
                // the body
                self.ind = saved.3 + 1;
                self.stmt_expr(body, &Dest::Return);
                let b = std::mem::take(&mut self.out);
                // the arms
                let mut ops = vec![];
                for (k, a) in arms.iter().enumerate() {
                    ops.push(js_str(&a.variant));
                    self.line(&format!("{}if ($op.TAG === {}) {{", if k == 0 { "" } else { "} else " }, js_str(&a.variant)));
                    self.ind += 1;
                    let mark = self.locals.len();
                    let mut binds = vec![];
                    for (i, p) in a.pats.iter().enumerate() {
                        self.pat_binds(p, &format!("$op._{i}"), &mut binds);
                    }
                    for (n, v) in binds {
                        let js = self.bind(&n);
                        self.line(&format!("const {js} = {v};"));
                    }
                    let k = self.bind(&a.k);
                    self.line(&format!("const {k} = $k;"));
                    self.conts.push(a.k.clone());
                    self.stmt_expr(&a.body, &Dest::Return);
                    self.conts.pop();
                    self.locals.truncate(mark);
                    self.ind -= 1;
                }
                self.line("}");
                let arms_js = std::mem::take(&mut self.out);
                let pad = "  ".repeat(saved.3);
                (self.gen_, self.conts, self.out, self.ind) = saved;
                let h = format!("$handle(function* () {{\n{b}{pad}}}, [{}], function* ($op, $k) {{\n{arms_js}{pad}}})", ops.join(", "));
                if self.gen_ { format!("(yield* {h})") } else { format!("$run({h})") }
            }
        }
    }

    fn help(&mut self, h: &'static str, v: &str) -> String {
        self.helpers.insert(h);
        format!("{h}({v})")
    }

    fn note_extern(&mut self, n: &str) {
        if let Some((_, Some(m), js)) = self.info.externs.get(n) {
            let root = js.split('.').next().unwrap_or(js).to_string();
            self.imports.entry(m.clone()).or_default().insert(root);
        }
    }

    fn binary(&mut self, id: Id, op: &str, l: &Expr, r: &Expr) -> String {
        let t = self.num(id);
        // (operands of the wrapping forms are inside `(..)` or a call: any precedence above `|` and `,` will do)
        let p = match (&t, op) {
            (Ty::I32 | Ty::U32 | Ty::I64, "+" | "-" | "*" | "/" | "%") => if op == "+" || op == "-" { bin_prec(op) } else { 3 },
            (Ty::F32, "+" | "-" | "*" | "/" | "%") => bin_prec(op),
            _ => bin_prec(op),
        };
        let (a, b) = (self.bin_operand(l, p, false), self.bin_operand(r, p, true));
        match op {
            "==" => return format!("{a} === {b}"),
            "!=" => return format!("{a} !== {b}"),
            "<" | "<=" | ">" | ">=" | "&&" | "||" => return format!("{a} {op} {b}"),
            _ => {}
        }
        match (t, op) {
            (Ty::I32, "+" | "-") => format!("({a} {op} {b}) | 0"),
            (Ty::I32, "*") => format!("Math.imul({a}, {b})"),
            (Ty::I32, "/") => self.help("$div_i32", &format!("{a}, {b}")),
            (Ty::I32, "%") => self.help("$rem_i32", &format!("{a}, {b}")),
            (Ty::U32, "+" | "-") => format!("({a} {op} {b}) >>> 0"),
            (Ty::U32, "*") => format!("Math.imul({a}, {b}) >>> 0"),
            (Ty::U32, "/") => self.help("$div_u32", &format!("{a}, {b}")),
            (Ty::U32, "%") => self.help("$rem_u32", &format!("{a}, {b}")),
            (Ty::I64, _) => format!("BigInt.asIntN(64, {a} {op} {b})"),
            // (f32: the f64 result rounded to f32 is exactly the f32 operation, for + - * / and %)
            (Ty::F32, _) => format!("Math.fround({a} {op} {b})"),
            _ => format!("{a} {op} {b}"),
        }
    }

    fn jsx_expr(&mut self, j: &Jsx) -> String {
        self.jsx = true;
        let tag = if j.tag.is_empty() {
            "_Fragment".to_string()
        } else if j.tag.chars().next().is_some_and(|c| c.is_uppercase()) {
            ident(&j.tag)
        } else {
            js_str(&j.tag)
        };
        let mut props = vec![];
        let mut key = None;
        for (a, x) in &j.attrs {
            let v = self.expr(x);
            if a == "key" {
                key = Some(v);
                continue;
            }
            let k = if a.contains('-') { js_str(a) } else { a.clone() };
            props.push(format!("{k}: {v}"));
        }
        let kids: Vec<String> = j
            .children
            .iter()
            .map(|c| match c {
                JsxChild::Text(t) => js_str(t),
                JsxChild::Expr(x) => self.expr(x),
            })
            .collect();
        let f = if kids.len() > 1 { "_jsxs" } else { "_jsx" };
        match kids.len() {
            0 => {}
            1 => props.push(format!("children: {}", kids[0])),
            _ => props.push(format!("children: [{}]", kids.join(", "))),
        }
        let p = if props.is_empty() { "{}".to_string() } else { format!("{{ {} }}", props.join(", ")) };
        match key {
            Some(k) => format!("{f}({tag}, {p}, {k})"),
            None => format!("{f}({tag}, {p})"),
        }
    }
}

/// The JS typed array of an element type
pub fn typed_js(el: &Ty) -> &'static str {
    match el {
        Ty::F32 => "Float32Array",
        Ty::I32 => "Int32Array",
        _ => "Float64Array",
    }
}

/// An f32 literal: the f32 nearest to it, written as its shortest f64 text (exact in JS)
fn f32_lit(f: &str) -> String {
    let v = f.parse::<f64>().unwrap_or(0.0) as f32 as f64;
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") }
}

/// The expression is one call `f(..)` / `a.b(..)` (its last `)` closes its first `(`)
fn single_call(s: &str) -> bool {
    let Some(i) = s.find('(') else { return false };
    if i == 0 || !s[..i].chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.') || !s.ends_with(')') {
        return false;
    }
    let mut d = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (k, c) in s[i..].char_indices() {
        // (parentheses inside a string literal are not structure)
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '(' => d += 1,
            ')' => {
                d -= 1;
                if d == 0 {
                    return i + k == s.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

/// A `break` / `continue` in `e` that leaves a loop outside `e` (not one of its own loops, not in a closure)
fn loop_exit(e: &Expr) -> Option<Pos> {
    fn walk(e: &Expr) -> Option<Pos> {
        match &e.kind {
            EK::Break | EK::Continue => Some(e.pos),
            EK::While(..) | EK::For(..) | EK::Closure(..) => None,
            EK::Block(stmts, tail) => stmts
                .iter()
                .find_map(|s| match s {
                    Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) | Stmt::Expr(x) => walk(x),
                })
                .or_else(|| tail.as_deref().and_then(walk)),
            EK::If(c, t, el) => walk(c).or_else(|| walk(t)).or_else(|| el.as_deref().and_then(walk)),
            EK::Match(s, arms) => walk(s).or_else(|| arms.iter().find_map(|a| walk(&a.1))),
            EK::Assign(_, x) | EK::Un(_, x) | EK::Field(x, _) | EK::Cast(x, _) => walk(x),
            EK::Bin(_, a, b) | EK::Index(a, b) => walk(a).or_else(|| walk(b)),
            EK::Call(f, a) => walk(f).or_else(|| a.iter().find_map(walk)),
            EK::Method(r, _, a) => walk(r).or_else(|| a.iter().find_map(walk)),
            EK::Array(xs) | EK::Tuple(xs) => xs.iter().find_map(walk),
            EK::Record(_, fs, b) => fs.iter().find_map(|f| walk(&f.1)).or_else(|| b.as_deref().and_then(walk)),
            _ => None,
        }
    }
    walk(e)
}

fn bin_prec(op: &str) -> u8 {
    match op {
        "*" | "/" | "%" => 13,
        "+" | "-" => 12,
        "<" | "<=" | ">" | ">=" => 10,
        "==" | "!=" => 9,
        "&&" => 5,
        "||" => 4,
        _ => 2,
    }
}

fn balanced(s: &str) -> bool {
    let mut d = 0i32;
    for c in s.chars() {
        match c {
            '(' => d += 1,
            ')' => {
                d -= 1;
                if d < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    d == 0
}

fn lit(l: &Lit, i64_: bool) -> String {
    match l {
        Lit::Int(n) if i64_ => format!("{n}n"),
        Lit::Int(n) => n.to_string(),
        Lit::Float(f) => f.clone(),
        Lit::Str(s) => js_str(s),
        Lit::Bool(b) => b.to_string(),
        Lit::Unit => "undefined".into(),
    }
}

// ---------------------------------------------------------------- .d.ts

thread_local! {
    /// Set when `ts` renders `Ty::Jsx`: the `.d.ts` then imports `ReactElement`
    static USES_JSX: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn ts(t: &Ty) -> String {
    match t {
        // (branded: `i32` is `number & { __zsc_num: "i32" }`, so TS keeps integers and floats apart too)
        Ty::I32 => "i32".into(),
        Ty::U32 => "u32".into(),
        Ty::F32 => "f32".into(),
        Ty::F64 => "f64".into(),
        Ty::Typed(e) => typed_js(e).into(),
        Ty::I64 => "bigint".into(),
        Ty::Str => "string".into(),
        Ty::Bool => "boolean".into(),
        Ty::Unit => "void".into(),
        // (imported under an alias, so a user type named ReactElement does not collide)
        Ty::Jsx => {
            USES_JSX.with(|u| u.set(true));
            "$ReactElement".into()
        }
        Ty::Array(a) => format!("readonly {}[]", ts_inner(a)),
        Ty::Tuple(ts_) => format!("readonly [{}]", ts_.iter().map(ts).collect::<Vec<_>>().join(", ")),
        Ty::Fn(a, r) => format!("({}) => {}", a.iter().enumerate().map(|(k, t)| format!("a{k}: {}", ts(t))).collect::<Vec<_>>().join(", "), ts(r)),
        Ty::Named(n, a) if a.is_empty() => n.clone(),
        Ty::Named(n, a) => format!("{n}<{}>", a.iter().map(ts).collect::<Vec<_>>().join(", ")),
        Ty::Param(p) => p.clone(),
        Ty::Var(_) => "unknown".into(),
    }
}

fn ts_inner(t: &Ty) -> String {
    let s = ts(t);
    if matches!(t, Ty::Fn(..)) { format!("({s})") } else { s }
}

/// The numeric brands: (name, what it holds, the JS check of the constructor)
pub const BRANDS: &[(&str, &str, &str)] = &[
    ("i32", "a 32-bit signed integer", "Number.isInteger(x) && x >= -2147483648 && x <= 2147483647"),
    ("u32", "a 32-bit unsigned integer", "Number.isInteger(x) && x >= 0 && x <= 4294967295"),
    ("f32", "a 32-bit float", "Object.is(Math.fround(x), x)"),
    ("f64", "a 64-bit float", "typeof x === \"number\""),
];

/// `w` as a whole word in `s`
fn uses_word(s: &str, w: &str) -> bool {
    let b = s.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    s.match_indices(w).any(|(k, _)| (k == 0 || !word(b[k - 1])) && b.get(k + w.len()).is_none_or(|c| !word(*c)))
}

/// The constructors of the brands the `.d.ts` uses, for the JS module: TS code makes an `i32` with `i32(x)`,
/// which checks (it does not convert: a TS number that is not an integer is a mistake at the boundary)
pub fn brand_runtime(used: &[&str]) -> String {
    let mut o = String::new();
    if used.is_empty() {
        return o;
    }
    o.push_str("// ---- constructors of the numeric brands of the .d.ts (for TS callers: they check, they do not convert)\n");
    for (n, what, check) in BRANDS {
        if used.contains(n) {
            let _ = writeln!(o, "export function {n}(x) {{\n  if (!({check})) throw new RangeError(\"not {what}: \" + x);\n  return x;\n}}\n");
        }
    }
    o
}

/// The `.d.ts`, and the numeric brands it uses
pub fn dts(items: &[Item], info: &Info) -> (String, Vec<&'static str>) {
    let mut o = String::new();
    USES_JSX.with(|u| u.set(false));
    for it in items {
        match it {
            Item::Record { name, params, .. } => {
                let r = &info.records[name];
                let g = if params.is_empty() { String::new() } else { format!("<{}>", params.join(", ")) };
                let _ = writeln!(o, "export interface {name}{g} {{");
                for (f, t) in &r.fields {
                    let _ = writeln!(o, "  readonly {f}: {};", ts(t));
                }
                o.push_str("}\n\n");
            }
            Item::Enum { name, params, .. } => {
                let e = &info.enums[name];
                let g = if params.is_empty() { String::new() } else { format!("<{}>", params.join(", ")) };
                let alts: Vec<String> = e
                    .variants
                    .iter()
                    .map(|(v, fs)| {
                        if e.unit_only() && !e.is_effect() {
                            js_str(v)
                        } else {
                            let mut s = format!("{{ readonly TAG: {}", js_str(v));
                            for (k, t) in fs.iter().enumerate() {
                                let _ = write!(s, "; readonly _{k}: {}", ts(t));
                            }
                            s.push_str(" }");
                            s
                        }
                    })
                    .collect();
                let _ = writeln!(o, "export type {name}{g} = {};\n", alts.join(" | "));
            }
            Item::Fn { name, public: true, generics, bounds, args, .. } if !bounds.is_empty() => {
                let s = &info.fns[name];
                for targs in all_instances(generics, bounds) {
                    let m: HashMap<String, Ty> = generics.iter().cloned().zip(targs.iter().cloned()).collect();
                    let ps: Vec<String> = args.iter().zip(&s.args).map(|((n, _), t)| format!("{}: {}", ident(n), ts(&subst_ty(t, &m)))).collect();
                    let _ = writeln!(o, "export declare function {}({}): {};", inst_name(name, &targs), ps.join(", "), ts(&subst_ty(&s.ret, &m)));
                }
            }
            Item::Fn { name, public: true, generics, args, .. } => {
                let s = &info.fns[name];
                let g = if generics.is_empty() { String::new() } else { format!("<{}>", generics.join(", ")) };
                let ps: Vec<String> = args.iter().zip(&s.args).map(|((n, _), t)| format!("{}: {}", ident(n), ts(t))).collect();
                let r = if info.effects[name].is_empty() {
                    ts(&s.ret)
                } else {
                    // (an effectful function is a generator of the effects' operations)
                    let ops: Vec<&String> = info.effects[name].iter().collect();
                    format!("Generator<{}, {}, unknown>", ops.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" | "), ts(&s.ret))
                };
                let _ = writeln!(o, "export declare function {}{g}({}): {r};", ident(name), ps.join(", "));
            }
            Item::WasmFn { name, public: true, .. } => {
                let (args, ret) = &info.wasm[name];
                let ps: Vec<String> = args.iter().map(|(n, w)| format!("{}: {}", ident(n), ts(&crate::check::wty_to_ty(w)))).collect();
                let _ = writeln!(o, "/** compiled to wasm */\nexport declare function {}({}): {};", ident(name), ps.join(", "), ts(&crate::check::wty_to_ty(ret)));
            }
            _ => {}
        }
    }
    let used: Vec<&'static str> = BRANDS.iter().map(|b| b.0).filter(|n| uses_word(&o, n)).collect();
    let mut h = String::from("// generated by zsc: the types of the module's exports\n");
    if USES_JSX.with(|u| u.replace(false)) {
        h.push_str("import type { ReactElement as $ReactElement } from \"react\";\n");
    }
    h.push('\n');
    if !used.is_empty() {
        h.push_str("// numbers keep their zustscript type: a brand on top of number (make one with `i32(1)` / `f64(0.5)`)\n");
        for (n, what, _) in BRANDS {
            if used.contains(n) {
                let _ = writeln!(h, "/** {what} */\nexport type {n} = number & {{ readonly __zsc_num: \"{n}\" }};\nexport declare function {n}(x: number): {n};");
            }
        }
        h.push('\n');
    }
    (h + &o, used)
}
