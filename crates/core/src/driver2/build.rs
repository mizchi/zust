//! The build part of driver2 (feature `build`): turning fns into machine code (lower), fork parallelism, objects, linking, workspaces.
//! Analysis and check (LSP) live in the parent driver2 and do not depend on the backend

use super::*;
use crate::acg::Target;
use crate::minclif::{DataDef, Reloc};
// (Fx hashing: deterministic and cheaper than the std map's random-seeded SipHash)
type HashMap<K, V> = crate::fxhash::FxMap<K, V>;

/// The fns (and instances) a Blob refers to: its calls and address-takings, and its vtables' slots
fn blob_fns(b: &Blob) -> Vec<u32> {
    let mut v: Vec<u32> = b.relocs.iter().filter_map(|r| match r.target { Target::Func(i) => Some(i), _ => None }).collect();
    v.extend(b.vtables.iter().flatten().copied());
    v
}

/// Turns the record into machine code and makes a Blob
pub(super) fn lower_blob(a: &Analysis, c: acg::Compiled, req: Vec<u32>) -> R<(Blob, Vec<u32>)> {
    let (code, relocs, traps) = crate::lower::lower(&c.rec, &c.late, a.prog.opt, a.cl)?;
    Ok((c.into_blob(code, relocs, traps), req))
}

/// Makes an ELF .o from source (no cache and no parallelism)
pub fn build_object(src0: &str, opt: bool) -> R<Vec<u8>> {
    let mut s = Session2::new(Options2 { opt, no_cache: true, jobs: 1, ..Default::default() });
    let a = s.analyze(src0)?;
    s.compile_object(&a, &mut Report2::default(), None)
}

/// Makes an executable from source (no cache and no parallelism)
pub fn build(src: &str, output: &std::path::Path, opt: bool, linker: Option<&str>) -> R<()> {
    let mut s = Session2::new(Options2 { opt, no_cache: true, jobs: 1, linker: linker.map(|l| l.to_string()), ..Default::default() });
    s.build(src, output).map(|_| ())
}


impl Session2 {
    /// Compiles ids (split across forked child processes when jobs > 1 and there are many). The results are in the same order as ids
    fn compile_many(&self, a: &Analysis, insts: &Mutex<Insts>, ids: &[u32]) -> R<Vec<Blob>> {
        // (at least 16 per child; ZUSTC_BUILD_PAR_MIN for tests: 1 splits nearly every wave)
        let per_job = std::env::var("ZUSTC_BUILD_PAR_MIN").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(16).max(1);
        let jobs = self.opts.jobs.min(ids.len() / per_job).max(1);
        if jobs <= 1 {
            let mut out = vec![];
            for &id in ids {
                out.push(compile_id(a, insts, id)?);
            }
            return Ok(out);
        }
        // Child process k compiles ids[i] (i % jobs == k) and writes a sequence of (index, failed?, contents) to the pipe
        let mut kids = vec![];
        for k in 0..jobs {
            let mut fds = [0i32; 2];
            // SAFETY: fds is 2 ints
            if unsafe { c_pipe(fds.as_mut_ptr()) } != 0 {
                return Err("pipe failed".into());
            }
            // SAFETY: the child only compiles and writes, then calls _exit
            let pid = fork_counted();
            if pid < 0 {
                return Err("fork failed".into());
            }
            if pid == 0 {
                unsafe { c_close(fds[0]) };
                let mut buf = vec![];
                let mut i = k;
                while i < ids.len() {
                    let at = buf.len();
                    crate::cache2::put_uv(&mut buf, i as u64);
                    match compile_id(a, insts, ids[i]) {
                        Ok(b) => {
                            let mut e = vec![];
                            {
                                let g = insts.lock().unwrap();
                                // (a Blob that names an instance the encoding cannot write is not sent: the parent
                                // compiles that fn again. A closure's fn-pointer instance (request_fnptr) is written as
                                // the closure's own, so the parent's code called the closure through a fn pointer
                                // with its environment's parameter, and got the arguments wrong (#192))
                                if !sendable(a, &*g, &blob_fns(&b)) {
                                    buf.truncate(at);
                                    i += jobs;
                                    continue;
                                }
                                crate::cache2::encode_blob(&b, &a.prog, &*g, &mut e);
                            }
                            buf.push(0);
                            crate::cache2::put_uv(&mut buf, e.len() as u64);
                            buf.extend_from_slice(&e);
                        }
                        Err(m) => {
                            buf.push(1);
                            crate::cache2::put_uv(&mut buf, m.len() as u64);
                            buf.extend_from_slice(m.as_bytes());
                        }
                    }
                    i += jobs;
                }
                write_all(fds[1], &buf);
                unsafe { c_close(fds[1]) };
                unsafe { c_exit(0) };
            }
            unsafe { c_close(fds[1]) };
            kids.push((pid, fds[0]));
        }
        let mut res: Vec<Option<Result<Vec<u8>, String>>> = (0..ids.len()).map(|_| None).collect();
        for &(pid, fd) in &kids {
            let d = read_all(fd);
            unsafe { c_close(fd) };
            let mut st = 0i32;
            unsafe { c_waitpid(pid, &mut st, 0) };
            let mut r = crate::cache2::Rd { d: &d, pos: 0 };
            while r.pos < d.len() {
                let (Some(i), Some(fail), Some(n)) = (r.uv(), r.u8(), r.uv()) else { break };
                let Some(body) = r.take(n as usize) else { break };
                let v = if fail == 0 { Ok(body.to_vec()) } else { Err(String::from_utf8_lossy(body).into_owned()) };
                if let Some(slot) = res.get_mut(i as usize) {
                    *slot = Some(v);
                }
            }
        }
        // (read back in order: instances the children requested are requested again here; the failure is the first one in ids order)
        let mut out = vec![];
        for (i, r) in res.into_iter().enumerate() {
            match r {
                Some(Ok(d)) => out.push(crate::cache2::decode_blob(&d, &a.prog, insts).ok_or("internal: cannot decode a compiled function")?),
                Some(Err(m)) => return Err(m),
                None => {
                    // (the child crashed: compile that fn here (if it gives an error, that error comes out))
                    out.push(compile_id(a, insts, ids[i])?);
                }
            }
        }
        Ok(out)
    }

    /// Makes the .o of an analyzed program (looks up the cache; if missing, compiles and puts it in the cache)
    fn compile_object(&mut self, a: &Analysis, rep: &mut Report2, tm: Option<&mut Timer>) -> R<Vec<u8>> {
        let mut live = a.prog.reachable(&a.refs);
        // (a function only a static points to (`static F: fn() = f;`) is compiled too)
        for c in &a.prog.consts {
            if c.stat.is_some() && c.ty_has_fn() {
                if let Ok(e) = crate::parse::parse_expr(&c.expr) {
                    let mut ids = vec![];
                    static_fns(&a.prog, &e, &mut ids, 0);
                    for f in ids {
                        if let Some(l) = live.get_mut(f as usize) {
                            *l = true;
                        }
                    }
                }
            }
        }
        let mut user_live: Vec<u32> = (0..a.prog.names.len()).filter(|&i| live[i] && concrete(&a.prog, i) && !a.is_ext(i as u32)).map(|i| i as u32).collect();
        let insts = Mutex::new(Insts::new(&a.prog));
        let mut blobs: FxMap<u32, Blob> = FxMap::default();
        let log_compiled = std::env::var_os("ZUSTC_LOG_COMPILED").is_some();
        let mut wave = user_live.clone();
        while !wave.is_empty() {
            let cur = wave.clone();
            let mut misses = vec![];
            let mut keys: FxMap<u32, Key> = FxMap::default();
            for &id in &wave {
                if blobs.contains_key(&id) || keys.contains_key(&id) {
                    continue;
                }
                let k = self.key_of(a, &insts, id);
                keys.insert(id, k);
                // (a fn with locations written into it is not used if its item's start line has changed)
                let lb = {
                    let g = insts.lock().unwrap();
                    a.line_base_for(&*g, id)
                };
                let raw = self.get(k);
                if log_compiled && raw.is_some() {
                    let d = crate::cache2::decode_blob(raw.as_ref().unwrap(), &a.prog, &insts);
                    match &d {
                        None => eprintln!("decode-fail {}", a.prog.names[insts.lock().unwrap().body_of(id) as usize]),
                        Some(b) if !(b.line_base.is_none() || b.line_base == lb) => eprintln!("line-mismatch {}", a.prog.names[insts.lock().unwrap().body_of(id) as usize]),
                        _ => {}
                    }
                }
                let hit = raw.and_then(|d| crate::cache2::decode_blob(&d, &a.prog, &insts)).filter(|b| b.line_base.is_none() || b.line_base == lb);
                match hit {
                    Some(b) => {
                        rep.fns_hit += 1;
                        blobs.insert(id, b);
                    }
                    None => misses.push(id),
                }
            }
            if log_compiled {
                let g = insts.lock().unwrap();
                for &id in &misses {
                    let b = g.body_of(id);
                    let args = g.get(id).map(|i| i.args.iter().map(|&t| a.prog.tn(t)).collect::<Vec<_>>().join(",")).unwrap_or_default();
                    eprintln!("compiled {}<{args}>", a.prog.names[b as usize]);
                }
            }
            let compiled = self.compile_many(a, &insts, &misses)?;
            rep.fns_compiled += misses.len();
            for (&id, b) in misses.iter().zip(compiled) {
                if !self.opts.no_cache {
                    let mut e = vec![];
                    {
                                let g = insts.lock().unwrap();
                                crate::cache2::encode_blob(&b, &a.prog, &*g, &mut e);
                            }
                    self.put(keys[&id], e);
                }
                blobs.insert(id, b);
            }
            wave = std::mem::take(&mut insts.lock().unwrap().pending);
            // (the fns that an expanded body calls are not reachable from the caller's names: also compile the non-generic fns that relocations point to)
            // (vtable slots too: the methods of one's own type made into `Box<dyn Shape>` are not necessarily reachable by name
            //  (the call is inside a fn of a dependency crate, whose body has been removed in the interface))
            for &id in &cur {
                if let Some(b) = blobs.get(&id) {
                    let fs = b.relocs.iter().filter_map(|r| if let Target::Func(f) = r.target { Some(f) } else { None });
                    let vs = b.vtables.iter().flat_map(|v| v.iter().copied());
                    for f in fs.chain(vs) {
                        if f != u32::MAX && acg::vt_lit(f).is_none() && (f as usize) < a.prog.names.len() && concrete(&a.prog, f as usize) && !a.is_ext(f) && !blobs.contains_key(&f) && !live[f as usize] {
                            live[f as usize] = true;
                            user_live.push(f);
                            wave.push(f);
                        }
                    }
                }
            }
            wave.retain(|id| !blobs.contains_key(id));
            let starts: Vec<u32> = {
                let g = insts.lock().unwrap();
                wave.iter().map(|&id| g.body_of(id)).collect()
            };
            let mut stack: Vec<u32> = starts.iter().flat_map(|&b| a.refs[b as usize].iter().copied()).collect();
            if starts.iter().any(|&b| matches!(a.prog.names[b as usize].as_str(), "__exr_display" | "__exr_debug")) {
                stack.extend((0..a.prog.names.len() as u32).filter(|&i| matches!(a.prog.sigs[i as usize].impl_trait.as_deref(), Some("Display" | "Debug"))));
            }
            while let Some(i) = stack.pop() {
                if std::mem::replace(&mut live[i as usize], true) {
                    continue;
                }
                if a.is_ext(i) {
                    continue;
                }
                stack.extend(a.refs[i as usize].iter().copied());
                if concrete(&a.prog, i as usize) && !blobs.contains_key(&i) {
                    user_live.push(i);
                    wave.push(i);
                }
            }
        }
        let insts = insts.into_inner().unwrap();
        if let Some(tm) = tm {
            tm.lap("codegen", format!("{} hit / {} compiled ({} generic instances, {} jobs)", rep.fns_hit, rep.fns_compiled, insts.list.len(), self.opts.jobs));
        }
        emit_object(a, &self.ctx, &insts, &user_live, &blobs, self.opts.module_writer)
    }

    /// Makes an executable from source. Does not link if the .o is the same as last time
    pub fn build(&mut self, src: &str, output: &std::path::Path) -> R<Report2> {
        let t0 = now_ns();
        let mut tm = Timer { on: self.opts.time, last: t0, start: t0 };
        let mut rep = Report2::default();
        let a = self.analyze(src).map_err(|e| self.unhoist_analysis_err(src, e))?;
        rep.items_parsed = a.n_parsed;
        tm.lap("front", format!("{} items, {} parsed", a.items.len(), a.n_parsed));
        let obj = self.compile_object(&a, &mut rep, Some(&mut tm)).map_err(|e| super::unhoist_err(&a, e))?;
        if let Some(e) = &a.late_err {
            return Err(super::unhoist_err(&a, e.clone()));
        }
        tm.lap("object", format!("{} bytes", obj.len()));
        rep.object_bytes = obj.len();
        // (wasm32: the module is the output, nothing to link)
        if self.opts.module_writer.is_some() {
            if !std::fs::read(output).is_ok_and(|old| old == obj) {
                std::fs::write(output, &obj).map_err(|e| format!("{}: {e}", output.display()))?;
                rep.linked = true;
            }
            if self.opts.time {
                eprintln!("  {:<10} {:>8.2} ms", "total", (now_ns() - tm.start) as f64 / 1e6);
            }
            return Ok(rep);
        }
        // (`x` -> `x.o`, `x.cold` -> `x.cold.o`: not `x.o`, which another output `x` would share, and whose object
        // equal to its own would skip relinking `x`)
        let obj_path = match output.extension() {
            Some(_) => {
                let mut o = output.as_os_str().to_owned();
                o.push(".o");
                std::path::PathBuf::from(o)
            }
            None => output.with_extension("o"),
        };
        // (and the output was linked from that object: not older than it)
        // (a regular file: a directory at the output path is not an executable)
        let mtime = |p: &std::path::Path| std::fs::metadata(p).ok().filter(|m| m.is_file()).and_then(|m| m.modified().ok());
        // (and readable: an output with mode 000 is relinked; opened only once it is a regular file, as a FIFO's open
        // would block)
        let same = std::fs::read(&obj_path).is_ok_and(|old| old == obj)
            && matches!((mtime(output), mtime(&obj_path)), (Some(e), Some(o)) if e >= o)
            && std::fs::File::open(output).is_ok();
        if same {
            tm.lap("link", "skipped (object unchanged)".to_string());
        } else {
            std::fs::write(&obj_path, &obj).map_err(|e| e.to_string())?;
            let libs = native_libs(&a.src, &a.items, &self.opts.lib_dirs);
            let mut objs: Vec<&std::path::Path> = vec![obj_path.as_path()];
            objs.extend(libs.iter().map(|l| l.as_path()));
            let used = crate::link::link_many(self.opts.linker.as_deref(), &objs, output)?;
            rep.linked = true;
            tm.lap("link", used);
        }
        if self.opts.time {
            eprintln!("  {:<10} {:>8.2} ms", "total", (now_ns() - tm.start) as f64 / 1e6);
        }
        Ok(rep)
    }
}

/// Compiles one fn (instance)
fn compile_id(a: &Analysis, insts: &Mutex<Insts>, id: u32) -> R<Blob> {
    compile_id_req(a, insts, id).map(|r| r.0)
}


fn emit_object(a: &Analysis, ctx: &CrateCtx, insts: &Insts, user_live: &[u32], blobs: &FxMap<u32, Blob>, writer: Option<ModuleWriter>) -> R<Vec<u8>> {
    let prog = &a.prog;
    let sym_of = |id: u32| -> &str {
        match insts.get(id) {
            Some(i) => &i.sig.symbol,
            None => &prog.sigs[id as usize].symbol,
        }
    };
    // The fns to emit: only those reachable from main / exports by following relocations and vtables
    let mut reach: FxSet<u32> = FxSet::default();
    let mut stack: Vec<u32> = user_live
        .iter()
        .copied()
        .filter(|&u| {
            let s = &prog.sigs[u as usize];
            s.is_main || s.export
        })
        .collect();
    // (a function a static points to: `static F: fn() = f;`)
    for c in &prog.consts {
        if c.stat.is_some() && c.ty_has_fn() {
            if let Ok(e) = crate::parse::parse_expr(&c.expr) {
                let mut ids = vec![];
                static_fns(prog, &e, &mut ids, 0);
                stack.extend(ids.into_iter().filter(|f| blobs.contains_key(f)));
            }
        }
    }
    while let Some(u) = stack.pop() {
        if !reach.insert(u) {
            continue;
        }
        let Some(b) = blobs.get(&u) else { continue };
        for r in &b.relocs {
            if let Target::Func(f) = r.target {
                stack.push(f);
            }
        }
        for vt in &b.vtables {
            stack.extend(vt.iter().copied().filter(|&f| f != u32::MAX && acg::vt_lit(f).is_none()));
        }
    }
    let mut placed: Vec<u32> = user_live.iter().copied().filter(|u| reach.contains(u) && blobs.contains_key(u)).collect();
    let mut order: Vec<u32> = (0..insts.list.len() as u32).map(|i| insts.n_fns + i).filter(|i| reach.contains(i) && blobs.contains_key(i)).collect();
    order.sort_by(|a, b| sym_of(*a).cmp(sym_of(*b)));
    // Instances that become the same machine code are emitted only once (ICF)
    let mut fold: FxMap<u32, u32> = FxMap::default();
    for _ in 0..4 {
        let canon = |f: u32, fold: &FxMap<u32, u32>| *fold.get(&f).unwrap_or(&f);
        let mut seen: HashMap<Vec<u8>, u32> = HashMap::default();
        let mut changed = false;
        for &iid in &order {
            if fold.contains_key(&iid) {
                continue;
            }
            let b = &blobs[&iid];
            let sig = &insts.get(iid).unwrap().sig;
            let mut key = Vec::with_capacity(b.code.len() + 64);
            key.extend_from_slice(&(b.code.len() as u32).to_le_bytes());
            key.extend_from_slice(&b.code);
            key.extend_from_slice(format!("{:?}", acg::clif_sig(sig)).as_bytes());
            for r in &b.relocs {
                key.extend_from_slice(&r.offset.to_le_bytes());
                key.push(r.kind);
                key.extend_from_slice(&r.addend.to_le_bytes());
                match r.target {
                    Target::Func(f) => {
                        key.push(0);
                        key.extend_from_slice(&canon(f, &fold).to_le_bytes());
                    }
                    Target::SelfOffset(o) => {
                        key.push(1);
                        key.extend_from_slice(&o.to_le_bytes());
                    }
                    Target::Data(k) => {
                        key.push(2);
                        key.extend_from_slice(&b.datas[k as usize]);
                        key.push(0xFF);
                    }
                    Target::Static(c) => {
                        key.push(3);
                        key.extend_from_slice(&c.to_le_bytes());
                    }
                    Target::Sym(ref n) => {
                        key.push(5);
                        key.extend_from_slice(n.as_bytes());
                    }
                    Target::Vtable(k) => {
                        key.push(4);
                        for &f in &b.vtables[k as usize] {
                            key.extend_from_slice(&canon(f, &fold).to_le_bytes());
                        }
                        key.push(0xFF);
                    }
                }
            }
            for &(o, i, mi) in &b.traps {
                key.extend_from_slice(&o.to_le_bytes());
                key.extend_from_slice(&i.to_le_bytes());
                if mi != u32::MAX {
                    key.extend_from_slice(&b.datas[mi as usize]);
                }
                key.push(0xFE);
            }
            match seen.get(&key) {
                Some(&r) => {
                    fold.insert(iid, r);
                    changed = true;
                }
                None => {
                    seen.insert(key, iid);
                }
            }
        }
        if !changed {
            break;
        }
    }
    order.retain(|i| !fold.contains_key(i));
    let resolve = |mut f: u32| {
        while let Some(&n) = fold.get(&f) {
            f = n;
        }
        f
    };
    placed.extend(order.iter().copied());
    let fname = |f: u32| -> String {
        let f = resolve(f);
        sym_of(f).to_string()
    };
    // statics: those referenced from code (and those referenced from their initial values)
    let mut datas: Vec<DataDef> = vec![];
    let mut anon: HashMap<Vec<u8>, String> = HashMap::default();
    let mut anon_data = |bytes: &[u8], datas: &mut Vec<DataDef>| -> String {
        if let Some(n) = anon.get(&bytes.to_vec()) {
            return n.clone();
        }
        let n = format!(".Lxd{}", anon.len());
        anon.insert(bytes.to_vec(), n.clone());
        datas.push((n.clone(), bytes.to_vec(), vec![], false));
        n
    };
    // (a library's panic table is exported as `_xT_<name>`: the executable's table points to it)
    let static_name = |ci: u32| match (&ctx.lib, prog.consts[ci as usize].name.as_str()) {
        // (a static of an extern block: the linked program's symbol)
        _ if prog.consts[ci as usize].expr.starts_with(crate::parse::EXTERN_STATIC_SYM) => {
            crate::parse::extern_static(&prog.consts[ci as usize].expr).map_or_else(String::new, |x| x.1.to_string())
        }
        (Some(l), "__EXR_TRAP_TABLE") => format!("_xT_{l}"),
        (None, "__EXR_TRAP_TABLE") | (_, "__EXR_EXE_TABLE") => "_xT__exe".to_string(),
        _ => format!("_xS{}", prog.consts[ci as usize].name),
    };
    let mut todo: Vec<u32> = vec![];
    for u in &placed {
        for r in &blobs[u].relocs {
            if let Target::Static(c) = r.target {
                todo.push(c);
            }
        }
    }
    // (an executable always emits the unwrap location that the dependency crates use)
    if ctx.lib.is_none() && !ctx.deps.is_empty() {
        todo.extend(prog.const_index.get("__EXR_CALLER").copied());
    }
    todo.sort_unstable();
    todo.dedup();
    let mut done_static: FxSet<u32> = FxSet::default();
    let mut trap_table: Option<u32> = None;
    let mut nested = 0usize;
    while let Some(ci) = todo.pop() {
        if !done_static.insert(ci) {
            continue;
        }
        let c = &prog.consts[ci as usize];
        // (an extern block's static is the linked program's: nothing to place)
        if c.expr.starts_with(crate::parse::EXTERN_STATIC_SYM) {
            continue;
        }
        if c.name == "__EXR_TRAP_TABLE" {
            trap_table = Some(ci);
            continue;
        }
        // (the executable's table: in an executable, the same as __EXR_TRAP_TABLE; in a library, an external one)
        if c.name == "__EXR_EXE_TABLE" {
            if ctx.lib.is_none() {
                todo.push(trap_table_ci(prog));
            }
            continue;
        }
        // (a dependency crate's statics are in its own .o; the unwrap location (__EXR_CALLER) is the executable's single one, shared by all)
        if a.is_ext_static(ci) || (ctx.lib.is_some() && c.name == "__EXR_CALLER") {
            continue;
        }
        let dv = crate::ast_consteval::eval_src(prog, &c.expr, c.ty).map_err(|e| format!("in static `{}`: {e}", c.name))?;
        // (nested anonymous data (the contents of `&[..]`) is emitted under a name)
        fn place(
            dv: crate::ast_consteval::DataVal,
            name: String,
            datas: &mut Vec<DataDef>,
            todo: &mut Vec<u32>,
            nested: &mut usize,
            static_name: &dyn Fn(u32) -> String,
            fname: &dyn Fn(u32) -> String,
        ) {
            let mut rs = vec![];
            for (off, r) in dv.relocs {
                match r {
                    crate::ast_consteval::DRef::Static(c) => {
                        todo.push(c);
                        rs.push((off, static_name(c), 0));
                    }
                    crate::ast_consteval::DRef::Anon(inner) => {
                        *nested += 1;
                        let n = format!(".Lxn{}", *nested);
                        place(inner, n.clone(), datas, todo, nested, static_name, fname);
                        rs.push((off, n, 0));
                    }
                    crate::ast_consteval::DRef::Func(f) => rs.push((off, fname(f), 0)),
                }
            }
            datas.push((name, dv.bytes, rs, false));
        }
        let n0 = datas.len();
        place(dv, static_name(ci), &mut datas, &mut todo, &mut nested, &static_name, &fname);
        // (a library's statics are exported)
        if (ctx.lib.is_some() && c.item < a.prog.prelude_from && !a.ext_items[c.item as usize]) || (ctx.lib.is_none() && !ctx.deps.is_empty() && c.name == "__EXR_CALLER") {
            if let Some(d) = datas[n0..].iter_mut().find(|d| d.0 == static_name(ci)) {
                d.3 = true;
            }
        }
    }
    // Fns and their relocations
    let mut funcs: Vec<(String, Vec<u8>, Vec<Reloc>, bool)> = vec![];
    let mut vts: HashMap<Vec<u32>, String> = HashMap::default();
    let mut fn_off: Vec<(u32, u64)> = vec![];
    let mut text_end = 0u64;
    for &uid in &placed {
        let blob = &blobs[&uid];
        let data_names: Vec<String> = blob.datas.iter().map(|d| anon_data(d, &mut datas)).collect();
        let mut vt_names = vec![];
        for vt in &blob.vtables {
            if let Some(n) = vts.get(vt) {
                vt_names.push(n.clone());
                continue;
            }
            let n = format!(".Lxv{}", vts.len());
            let mut rs = vec![];
            let mut bytes = vec![0u8; 8 * vt.len().max(1)];
            for (i, &f) in vt.iter().enumerate() {
                if let Some(n) = acg::vt_lit(f) {
                    bytes[8 * i..8 * i + 8].copy_from_slice(&n.to_le_bytes());
                    continue;
                }
                if f != u32::MAX && (reach.contains(&resolve(f)) || prog.sigs.get(f as usize).is_some_and(|s| s.is_extern) || a.is_ext(f)) {
                    rs.push((8 * i as u32, fname(f), 0));
                }
            }
            datas.push((n.clone(), bytes, rs, false));
            vts.insert(vt.clone(), n.clone());
            vt_names.push(n);
        }
        let relocs: Vec<Reloc> = blob
            .relocs
            .iter()
            .map(|r| Reloc {
                offset: r.offset,
                name: match r.target {
                    Target::Func(t) => fname(t),
                    Target::SelfOffset(_) => sym_of(uid).to_string(),
                    Target::Data(k) => data_names[k as usize].clone(),
                    Target::Static(c) => static_name(c),
                    Target::Vtable(k) => vt_names[k as usize].clone(),
                    Target::Sym(ref n) => n.clone(),
                },
                kind: match r.kind {
                    5 => 9,
                    6 => 2,
                    7 => 1,
                    _ => 4,
                },
                addend: match r.target {
                    Target::SelfOffset(o) => r.addend + o as i64,
                    _ => r.addend,
                },
            })
            .collect();
        let s = &prog.sigs.get(uid as usize);
        let global = insts.get(uid).is_none() && s.is_some_and(|s| s.is_main || s.export);
        text_end = (text_end + 15) / 16 * 16;
        fn_off.push((uid, text_end));
        text_end += blob.code.len() as u64;
        funcs.push((sym_of(uid).to_string(), blob.code.clone(), relocs, global));
    }
    // The panic trap table (same format as driver.rs)
    // (an executable with dependency crates emits the table even if it does not use it: the dependency crates' .o refer to `_xT__exe`)
    if trap_table.is_some() || ctx.lib.is_some() || !ctx.deps.is_empty() {
        let tname = match (trap_table, &ctx.lib) {
            (Some(ci), _) => static_name(ci),
            (None, Some(l)) => format!("_xT_{l}"),
            (None, None) => "_xT__exe".to_string(),
        };
        let mut bytes: Vec<u8> = vec![0; 16];
        let uv = |out: &mut Vec<u8>, mut v: u64| {
            while v >= 0x80 {
                out.push(v as u8 | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        };
        let mut msgs: Vec<Vec<u8>> = vec![];
        let mut fbytes: Vec<u8> = vec![];
        let mut nf = 0u64;
        let (mut prev_fn, mut prev_line) = (0u64, 0i64);
        for &(uid, fo) in &fn_off {
            let blob = &blobs[&uid];
            if blob.traps.is_empty() {
                continue;
            }
            nf += 1;
            // (the locations of a dependency crate's closed fns are lines in that crate's text: add the position in this source)
            let shift = a.line_shift(insts, uid) as i64;
            uv(&mut fbytes, fo - prev_fn);
            prev_fn = fo;
            uv(&mut fbytes, blob.traps.len() as u64);
            let mut prev_site = 0u64;
            for &(off, info, mi) in &blob.traps {
                let gm = if mi == u32::MAX {
                    0
                } else {
                    let b = &blob.datas[mi as usize];
                    match msgs.iter().position(|x| x == b) {
                        Some(k) => k as u64 + 1,
                        None => {
                            msgs.push(b.clone());
                            msgs.len() as u64
                        }
                    }
                };
                let (kind, mut line, col) = ((info >> 27) as u64, ((info >> 11) & 0xFFFF) as i64, (info & 0x7FF) as u64);
                if shift > 0 && line > 0 && line < PRELUDE_LINE as i64 {
                    line += shift;
                }
                uv(&mut fbytes, off as u64 - prev_site);
                prev_site = off as u64;
                uv(&mut fbytes, kind | (gm << 4));
                let d = line - prev_line;
                prev_line = line;
                uv(&mut fbytes, ((d << 1) ^ (d >> 63)) as u64);
                uv(&mut fbytes, col);
            }
        }
        // (the file map: line of the joined source → file; from PRELUDE_LINE on it is the prelude)
        let mut segs: Vec<(u32, String, u32)> = ctx.map.segs.iter().map(|&(st, fi, first)| (st, ctx.map.files.get(fi as usize).cloned().unwrap_or_else(|| "src/main.rs".to_string()), first)).collect();
        if segs.is_empty() {
            segs.push((1, "src/main.rs".to_string(), 1));
        }
        if a.prelude_line0 > 0 && (a.prelude_line0 as u32) < PRELUDE_LINE {
            segs.push((PRELUDE_LINE + 1, "<zustc prelude>".to_string(), 1));
        }
        uv(&mut bytes, segs.len() as u64);
        for (st, n, first) in &segs {
            uv(&mut bytes, *st as u64);
            uv(&mut bytes, *first as u64);
            uv(&mut bytes, n.len() as u64);
            bytes.extend_from_slice(n.as_bytes());
        }
        // (column shifts: (line, column, shift (zigzag)))
        uv(&mut bytes, ctx.map.colfix.len() as u64);
        for &(l, c, d) in &ctx.map.colfix {
            uv(&mut bytes, l as u64);
            uv(&mut bytes, c as u64);
            let d = d as i64;
            uv(&mut bytes, ((d << 1) ^ (d >> 63)) as u64);
        }
        uv(&mut bytes, msgs.len() as u64);
        for mb in &msgs {
            uv(&mut bytes, mb.len() as u64);
            bytes.extend_from_slice(mb);
        }
        uv(&mut bytes, nf);
        bytes.extend_from_slice(&fbytes);
        let mut rs = match funcs.first() {
            Some(f) => vec![(0u32, f.0.clone(), 0i64)],
            None => vec![],
        };
        // (an executable: at byte 8, the list of the dependency crates' tables [count, address *])
        if ctx.lib.is_none() && !ctx.deps.is_empty() {
            let mut ext = vec![0u8; 8 * (1 + ctx.deps.len())];
            ext[..8].copy_from_slice(&(ctx.deps.len() as u64).to_le_bytes());
            let ers: Vec<(u32, String, i64)> = ctx.deps.iter().enumerate().map(|(i, d)| (8 * (i as u32 + 1), format!("_xT_{d}"), 0i64)).collect();
            datas.push((".Lxext".to_string(), ext, ers, false));
            rs.push((8, ".Lxext".to_string(), 0));
        }
        // (a library's table has the crate's tag at byte 8)
        if let Some(l) = &ctx.lib {
            bytes[8..16].copy_from_slice(&(crate_tag(l) as u64).to_le_bytes());
        }
        datas.push((tname, bytes, rs, true));
    }
    match writer {
        Some(w) => w(&funcs, &datas),
        None => Ok(crate::minclif::write_object_full(&funcs, &datas)),
    }
}

/// A wasm32 crate's object: the functions and data of the module it is a part of, as `w(&funcs, &datas)` takes them
/// (the module writer's arguments), for a bin's "link" to write the module whole
fn write_parts(funcs: &[(String, Vec<u8>, Vec<Reloc>, bool)], datas: &[DataDef]) -> Result<Vec<u8>, String> {
    fn uv(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    fn bytes(out: &mut Vec<u8>, b: &[u8]) {
        uv(out, b.len() as u64);
        out.extend_from_slice(b);
    }
    let zz = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
    let mut out = PARTS_MAGIC.to_vec();
    uv(&mut out, funcs.len() as u64);
    for (n, code, rs, g) in funcs {
        bytes(&mut out, n.as_bytes());
        bytes(&mut out, code);
        uv(&mut out, rs.len() as u64);
        for r in rs {
            uv(&mut out, r.offset as u64);
            bytes(&mut out, r.name.as_bytes());
            uv(&mut out, r.kind as u64);
            uv(&mut out, zz(r.addend));
        }
        out.push(*g as u8);
    }
    uv(&mut out, datas.len() as u64);
    for (n, d, rs, g) in datas {
        bytes(&mut out, n.as_bytes());
        bytes(&mut out, d);
        uv(&mut out, rs.len() as u64);
        for (o, rn, a) in rs {
            uv(&mut out, *o as u64);
            bytes(&mut out, rn.as_bytes());
            uv(&mut out, zz(*a));
        }
        out.push(*g as u8);
    }
    Ok(out)
}

const PARTS_MAGIC: &[u8] = b"zust-wasm-parts 1\n";

type Parts = (Vec<(String, Vec<u8>, Vec<Reloc>, bool)>, Vec<DataDef>);

fn read_parts(d: &[u8]) -> Option<Parts> {
    let d = d.strip_prefix(PARTS_MAGIC)?;
    let mut r = crate::cache2::Rd { d, pos: 0 };
    let unzz = |v: u64| ((v >> 1) as i64) ^ -((v & 1) as i64);
    let s = |r: &mut crate::cache2::Rd| -> Option<String> {
        let n = r.uv()? as usize;
        Some(String::from_utf8_lossy(r.take(n)?).into_owned())
    };
    let mut funcs = vec![];
    for _ in 0..r.uv()? {
        let n = s(&mut r)?;
        let cl = r.uv()? as usize;
        let code = r.take(cl)?.to_vec();
        let mut rs = vec![];
        for _ in 0..r.uv()? {
            let offset = r.uv()? as u32;
            let name = s(&mut r)?;
            let kind = r.uv()? as u32;
            let addend = unzz(r.uv()?);
            rs.push(Reloc { offset, name, kind, addend });
        }
        funcs.push((n, code, rs, r.u8()? != 0));
    }
    let mut datas = vec![];
    for _ in 0..r.uv()? {
        let n = s(&mut r)?;
        let dl = r.uv()? as usize;
        let bytes = r.take(dl)?.to_vec();
        let mut rs = vec![];
        for _ in 0..r.uv()? {
            let o = r.uv()? as u32;
            let rn = s(&mut r)?;
            rs.push((o, rn, unzz(r.uv()?)));
        }
        datas.push((n, bytes, rs, r.u8()? != 0));
    }
    Some((funcs, datas))
}

/// One module's functions and data from its crates' parts. A name a crate keeps to itself (a generic instance, a
/// string's data) may be in two crates: it gets the crate's number (`name@1`), in its definition and in the
/// relocations of that crate that name it
fn merge_parts(parts: Vec<Parts>) -> R<Parts> {
    // (the global names, and which part defines each: two parts may not both define one, and a local name of the bin's
    //  part (the first) that a dependency defines globally is renamed too, so one name never has two bodies)
    let mut global_of: FxMap<String, usize> = FxMap::default();
    for (ci, (fs, ds)) in parts.iter().enumerate() {
        for n in fs.iter().filter(|f| f.3).map(|f| &f.0).chain(ds.iter().filter(|d| d.3).map(|d| &d.0)) {
            if let Some(&cj) = global_of.get(n) {
                if cj != ci {
                    return Err(format!("wasm32: the symbol `{n}` is defined by two crates of the build"));
                }
            }
            global_of.insert(n.clone(), ci);
        }
    }
    let mut funcs = vec![];
    let mut datas = vec![];
    for (ci, (fs, ds)) in parts.into_iter().enumerate() {
        let local: FxSet<String> = fs.iter().filter(|f| !f.3).map(|f| f.0.clone()).chain(ds.iter().filter(|d| !d.3).map(|d| d.0.clone())).collect();
        let rn = |n: String| if local.contains(&n) && (ci > 0 || global_of.contains_key(&n)) { format!("{n}@{ci}") } else { n };
        for (n, code, rs, g) in fs {
            let rs = rs.into_iter().map(|r| Reloc { name: rn(r.name), ..r }).collect();
            funcs.push((rn(n), code, rs, g));
        }
        for (n, d, rs, g) in ds {
            let rs = rs.into_iter().map(|(o, x, a)| (o, rn(x), a)).collect();
            datas.push((rn(n), d, rs, g));
        }
    }
    Ok((funcs, datas))
}

fn trap_table_ci(prog: &Program) -> u32 {
    prog.const_index.get("__EXR_TRAP_TABLE").copied().unwrap_or(0)
}

/// One crate of a workspace
#[derive(Clone, Debug)]
pub struct CrateSpec {
    pub name: String,
    /// The root file (lib.rs / main.rs)
    pub root: std::path::PathBuf,
    /// The path to display (in panic and error locations)
    pub display: String,
    pub lib: bool,
    /// The names of the directly depended-on crates
    pub deps: Vec<String>,
    /// The enabled features (`features=a,b`)
    pub features: Vec<String>,
    /// (a bin: the name of the executable, when it is not `name`. A Cargo package's lib and bin share the package's
    /// name, so zurgo gives the bin another crate name and keeps the executable's here)
    pub exe: Option<String>,
    /// more cfgs (a build script's `rustc-cfg`): `name`, or `name = "value"` (`cfg=name`, `cfg=name=value`)
    pub cfgs: Vec<(String, Option<String>)>,
    /// the compile-time environment for `env!` / `option_env!` (`env=KEY=VALUE`): cargo's `CARGO_PKG_*`, `OUT_DIR`,
    /// a build script's `rustc-env`
    pub env: Vec<(String, String)>,
    /// (a proc-macro crate, which zurgo builds as a stub library of its name: the dylib rustc built from it. The
    /// crates that reach it use its macros (crate::pm, the toolchain's proc-macro server runs them). `pm=PATH`)
    pub proc_macro: Option<std::path::PathBuf>,
    /// `test=true`: a test crate (`zustc --test`): its `#[test]` functions with a harness for `main`
    /// (crate::test_harness). The crate's cfgs then include `test`
    pub test: bool,
}

/// A value for the workspace file: `%`, `#`, whitespace and other control characters as `%XX`
pub fn ws_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '#' || c.is_whitespace() || c.is_control() {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn ws_unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: &u8| (*c as char).to_digit(16);
        if b[i] == b'%' {
            if let (Some(h), Some(l)) = (b.get(i + 1).and_then(hex), b.get(i + 2).and_then(hex)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads a workspace description file. One crate per line: `lib <name> <root file> [deps...]` / `bin <name> <root file> [deps...]`
/// (in place of a dependency, `features=a,b` enables features; a bin's `exe=NAME` is the name of the executable; `cfg=name` / `cfg=name=value`
/// is a cfg, `env=KEY=VALUE` is a value for `env!`: `%`, `#` and whitespace in the value are `%XX` (ws_escape); `pm=PATH` is a proc-macro crate's dylib)
/// (everything from `#` on is a comment; paths are relative to the description file's directory)
pub fn read_workspace(path: &std::path::Path) -> R<Vec<CrateSpec>> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let mut out: Vec<CrateSpec> = vec![];
    for (i, line) in text.lines().enumerate() {
        let line = match line.find('#') {
            Some(k) => &line[..k],
            None => line,
        };
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.is_empty() {
            continue;
        }
        if w.len() < 3 || (w[0] != "lib" && w[0] != "bin") {
            return Err(format!("{}:{}: expected `lib|bin <name> <root> [deps...]`", path.display(), i + 1));
        }
        out.push(CrateSpec {
            name: w[1].to_string(),
            root: dir.join(w[2]),
            display: w[2].to_string(),
            lib: w[0] == "lib",
            deps: w[3..].iter().filter(|s| !s.contains('=')).map(|s| s.to_string()).collect(),
            features: w[3..]
                .iter()
                .filter_map(|s| s.strip_prefix("features="))
                .flat_map(|s| s.split(','))
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect(),
            exe: w[3..].iter().find_map(|s| s.strip_prefix("exe=")).map(str::to_string),
            cfgs: w[3..]
                .iter()
                .filter_map(|s| s.strip_prefix("cfg="))
                .map(|s| match s.split_once('=') {
                    Some((k, v)) => (ws_unescape(k), Some(ws_unescape(v))),
                    None => (ws_unescape(s), None),
                })
                .collect(),
            env: w[3..].iter().filter_map(|s| s.strip_prefix("env=")).filter_map(|s| s.split_once('=')).map(|(k, v)| (ws_unescape(k), ws_unescape(v))).collect(),
            proc_macro: w[3..].iter().find_map(|s| s.strip_prefix("pm=")).map(|p| dir.join(ws_unescape(p))),
            test: w[3..].contains(&"test=true"),
        });
    }
    Ok(out)
}

/// The result of a workspace build (per crate)
#[derive(Clone, Debug, Default)]
pub struct WsReport {
    pub crates: Vec<(String, Report2)>,
    /// The number of .o files rewritten
    pub objects_written: usize,
    pub linked: usize,
}

/// Sorts in dependency order (what is depended on comes first)
fn topo(crates: &[CrateSpec]) -> R<Vec<usize>> {
    let mut order: Vec<usize> = vec![];
    let mut state = vec![0u8; crates.len()];
    fn visit(i: usize, crates: &[CrateSpec], state: &mut Vec<u8>, order: &mut Vec<usize>) -> R<()> {
        if state[i] == 2 {
            return Ok(());
        }
        if state[i] == 1 {
            return Err(format!("dependency cycle at crate `{}`", crates[i].name));
        }
        state[i] = 1;
        for d in &crates[i].deps {
            let j = crates.iter().position(|c| &c.name == d).ok_or(format!("crate `{}`: unknown dependency `{d}`", crates[i].name))?;
            if !crates[j].lib {
                return Err(format!("crate `{}`: dependency `{d}` is not a lib", crates[i].name));
            }
            visit(j, crates, state, order)?;
        }
        state[i] = 2;
        order.push(i);
        Ok(())
    }
    for i in 0..crates.len() {
        visit(i, crates, &mut state, &mut order)?;
    }
    Ok(order)
}

/// The hash of the parts of a crate's source (`pub mod name { .. }`) that affect the compilation of its users.
/// The bodies of non-generic fns are not compiled on the user's side (they are called by symbol), so only their signatures go in.
/// For generic fns, trait members, members of generic impls, `const fn` and `-> impl Trait`, the bodies too (the user's side instantiates / evaluates them),
/// and the lines too (panic locations)
pub fn interface_hash(text: &str) -> Key {
    let mut h = KeyHasher::new();
    // (flatten the inside of `pub mod name {` and the last `}`)
    let body = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => &text[a + 1..b],
        _ => text,
    };
    let flat = lexer::flatten_modules(body);
    let inner: &str = flat.as_deref().unwrap_or(body);
    let items = match lexer::split(inner) {
        Ok(i) => i,
        Err(_) => {
            h.add(text);
            return h.finish();
        }
    };
    for it in &items {
        let t = &inner[it.start..it.end];
        let header_generic = match it.kind {
            ItemKind::ImplMember(hi) => items.get(hi as usize).is_some_and(|hd| inner[hd.start..hd.end].contains('<')),
            ItemKind::TraitMember(_) => true,
            _ => false,
        };
        let sig_end = t.find('{').unwrap_or(t.len());
        let sig = &t[..sig_end];
        let is_fn = sig.contains("fn ");
        let generic_fn = match (sig.find("fn "), sig.find('(')) {
            (Some(a), Some(b)) if a < b => sig[a..b].contains('<'),
            _ => true,
        };
        let concrete = is_fn && !header_generic && !generic_fn && !sig.contains("impl ") && !sig.contains("const fn") && sig_end < t.len();
        if concrete {
            h.add(&1u8);
            h.add(sig);
        } else {
            h.add(&2u8);
            h.add(t);
            if is_fn {
                h.add(&(it.line as u64));
            }
        }
    }
    h.finish()
}

/// A hex string (in place of u128::from_str_radix: zustc's std does not have it)
fn parse_hex(s: &str) -> Option<u128> {
    let mut v: u128 = 0;
    for c in s.bytes() {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => return None,
        };
        v = (v << 4) | d as u128;
    }
    Some(v)
}

fn key_hex(k: Key) -> String {
    format!("{:016x}{:016x}", (k >> 64) as u64, k as u64)
}

/// Creates a directory (and its parents) (in place of std::fs::create_dir_all: zustc's std does not have it)
fn mkdir_all(dir: &std::path::Path) -> R<()> {
    let s = format!("{}", dir.display());
    let b = s.as_bytes();
    for i in 1..=b.len() {
        if i == b.len() || b[i] == b'/' {
            let p = format!("{}\0", &s[..i]);
            // SAFETY: a NUL-terminated string
            unsafe { c_mkdir(p.as_bytes().as_ptr(), 0o755) };
        }
    }
    if dir.is_dir() { Ok(()) } else { Err(format!("{}: cannot create the directory", dir.display())) }
}

/// A crate's features, cfgs and compile-time environment, for its fingerprints (and its dependents': a `cfg!` or an
/// `env!($k)` in a macro_rules! body is only evaluated after expansion, so the text alone does not show a change of
/// them)
fn add_crate_cfg(h: &mut KeyHasher, c: &CrateSpec) {
    // (the lists as they are: no string built for each, as every no-op build computes this)
    h.add(&c.features);
    h.add(&c.cfgs);
    h.add(&c.env);
    // (a proc macro's dylib: also when it is rebuilt in place)
    if let Some(p) = &c.proc_macro {
        h.add(p.as_os_str());
        if let Ok(m) = std::fs::metadata(p) {
            h.add(&m.len());
            h.add(&m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos()));
        }
    }
}

/// `--test`'s crate: its source with the harness (crate::test_harness). A `#[test]` it cannot see (made by a
/// `macro_rules!`) is not run: the build says so
fn with_harness(src: &str, display: &str, cfg: &crate::cfg::CfgSet) -> String {
    // (the tests are collected from the source with its `macro_rules!` expanded, as the build expands them: a test a
    //  macro makes runs too. When the expansion fails here, the build reports it; the harness counts what it saw)
    let expanded = if src.contains("macro_rules!") { crate::macros::expand_cfg(src, Some(cfg)).ok().flatten() } else { None };
    let (out, unlisted) = crate::test_harness::apply_with(src, expanded.as_deref(), cfg.wasm);
    if unlisted > 0 {
        eprintln!("warning: {display}: {unlisted} `#[test]` function(s) made by a macro (counting each `#[$attr]` in a macro's body, which may be one) are not run yet (mizchi/zust#96)");
    }
    out
}

fn n_lines(s: &str) -> u32 {
    s.as_bytes().iter().filter(|&&c| c == b'\n').count() as u32
}

impl Session2 {
    /// Builds one crate (multiple files): follows `mod x;` from the root file
    pub fn build_path(&mut self, root: &std::path::Path, display: &str, output: &std::path::Path) -> R<Report2> {
        let (src, map) = crate::srcmap::load_crate(root, display, &self.opts.cfg_with(&self.opts.features))?;
        let src = if self.opts.test { with_harness(&src, display, &self.opts.cfg_with(&self.opts.features)) } else { src };
        self.ctx = CrateCtx { map: map.clone(), ..Default::default() };
        // (with one file, locations stay as `line:col:` (the caller adds the file name))
        let r = self.build(&src, output).map_err(|e| if map.files.len() > 1 { self.ctx.map.relocate_err(&e) } else { e });
        self.ctx = CrateCtx::default();
        self.last_map = Some(map);
        r
    }

    /// Compiles the single workspace crate i and writes the .o, the cache and the fingerprint. (result, whether the .o changed, time (ns))
    fn build_one(
        &mut self,
        crates: &[CrateSpec],
        loaded: &[Option<(String, crate::srcmap::SrcMap)>],
        dep_loaded: &[Option<(String, crate::srcmap::SrcMap)>],
        prune: (&[Option<Vec<crate::stub::StubItem>>], Option<&[(usize, usize)]>),
        i: usize,
        deps: &[usize],
        fp: &str,
        out_dir: &std::path::Path,
    ) -> R<(Report2, bool, u64)> {
        let c = &crates[i];
        let t1 = now_ns();
        let fp_path = out_dir.join(format!("{}.fp", c.name).as_str());
        let op = out_dir.join(format!("{}.o", c.name).as_str());
        let cp = out_dir.join(format!("{}.crcache2", c.name).as_str());
        if !self.opts.no_cache {
            self.disk = crate::cache2::Cache2::open(cp.as_path());
        }
        // The source: own + the dependency crates (appended after it)
        let (own, own_map) = loaded[i].clone().unwrap();
        let mut src = own.clone();
        let mut map = own_map.clone();
        src.push('\n');
        let no_prune = std::env::var_os("ZUSTC_NO_DEP_PRUNE").is_some();
        let mut ranges: Vec<(u32, u32, Vec<usize>, u32)> = vec![];
        for &j in deps {
            let (t0, m) = dep_loaded[j].as_ref().unwrap();
            // (items not reachable by name are blanked out: flattening, summaries, Program and the prelude selection then get only what is used)
            let pruned;
            let t: &str = match (prune.1, prune.0.get(j).and_then(|x| x.as_ref())) {
                (Some(reach), Some(its)) if !no_prune => {
                    let mut keep = vec![false; its.len()];
                    for &(jj, k) in reach {
                        if jj == j {
                            keep[k] = true;
                        }
                    }
                    pruned = crate::stub::prune(t0, its, &keep);
                    pruned.as_str()
                }
                _ => t0.as_str(),
            };
            let at = n_lines(&src) + 1;
            let f0 = map.files.len() as u32;
            for &(s, f, first) in &m.segs {
                map.segs.push((s + at - 1, f + f0, first));
            }
            map.files.extend(m.files.iter().cloned());
            src.push_str(t);
            src.push('\n');
            ranges.push((at, n_lines(&src) + 1, vec![], crate_tag(&crates[j].name)));
        }
        // (the crates visible from dependency crate j: j itself and j's transitive dependencies)
        for (k, &j) in deps.iter().enumerate() {
            let mut seen = vec![false; crates.len()];
            let mut st = vec![j];
            seen[j] = true;
            while let Some(x) = st.pop() {
                for d in &crates[x].deps {
                    if let Some(y) = crates.iter().position(|c| &c.name == d) {
                        if !seen[y] {
                            seen[y] = true;
                            st.push(y);
                        }
                    }
                }
            }
            let mut vis: Vec<usize> = vec![];
            for (q, &y) in deps.iter().enumerate() {
                if seen[y] {
                    vis.push(q);
                }
            }
            ranges[k].2 = vis;
        }
        self.ctx = CrateCtx {
            lib: if c.lib { Some(c.name.clone()) } else { None },
            own_lines: n_lines(&own) + 1,
            map: map.clone(),
            deps: deps.iter().map(|&j| crates[j].name.clone()).collect(),
            features: Some(c.features.clone()),
            cfgs: c.cfgs.clone(),
            env: c.env.clone(),
            dep_ranges: ranges,
        };
        let mut r = Report2::default();
        let a = self.analyze(&src).map_err(|e| format!("crate `{}`: {}", c.name, self.ctx.map.relocate_err(&e)))?;
        r.items_parsed = a.n_parsed;
        let obj = self.compile_object(&a, &mut r, None).map_err(|e| format!("crate `{}`: {}", c.name, self.ctx.map.relocate_err(&e)))?;
        r.object_bytes = obj.len();
        if !self.opts.no_cache {
            self.save_cache_keep(cp.as_path())?;
            std::fs::write(fp_path.as_path(), fp).map_err(|e| format!("{}: {e}", fp_path.display()))?;
        } else {
            // (the fingerprints of an earlier cached build no longer describe the object written here: without them the
            // next cached build compiles and links again)
            let _ = std::fs::remove_file(fp_path.as_path());
            let _ = std::fs::remove_file(out_dir.join(format!("{}.fp2", c.name).as_str()));
        }
        let mut changed = false;
        if !std::fs::read(op.as_path()).is_ok_and(|old| old == obj) {
            std::fs::write(op.as_path(), &obj).map_err(|e| format!("{}: {e}", op.display()))?;
            changed = true;
        }
        self.ctx = CrateCtx::default();
        Ok((r, changed, now_ns() - t1))
    }

    /// The precise fingerprint (build_workspace's .fp2): the hash of the compiler, the settings, own source (after expansion), the names and
    /// files of the dependency crates, and the items of the dependency crates' interfaces reachable by name from own identifiers
    #[allow(clippy::too_many_arguments)]
    fn precise_fp(
        &self,
        c: &CrateSpec,
        own: &str,
        own_map: &crate::srcmap::SrcMap,
        deps: &[usize],
        crates: &[CrateSpec],
        loaded: &[Option<(String, crate::srcmap::SrcMap)>],
        dep_items: &[Option<Vec<crate::stub::StubItem>>],
    ) -> (String, Vec<(usize, usize)>) {
        // (name → (crate, item))
        let mut by_name: FxMap<String, Vec<(usize, usize)>> = FxMap::default();
        for &j in deps {
            for (k, it) in dep_items[j].as_ref().unwrap().iter().enumerate() {
                for nm in &it.names {
                    by_name.entry(nm.clone()).or_default().push((j, k));
                }
            }
        }
        let mut seen_name: FxSet<String> = FxSet::default();
        let mut work: Vec<String> = vec![];
        let b = own.as_bytes();
        for t in lexer::tokenize(b) {
            if t.0 == 0 {
                let w = String::from_utf8_lossy(&b[t.1..t.2]).into_owned();
                if seen_name.insert(w.clone()) {
                    work.push(w);
                }
            }
        }
        let mut reached: Vec<(usize, usize)> = vec![];
        let mut seen_item: FxSet<(usize, usize)> = FxSet::default();
        while let Some(w) = work.pop() {
            let Some(v) = by_name.get(&w) else { continue };
            for &(j, k) in v {
                if !seen_item.insert((j, k)) {
                    continue;
                }
                reached.push((j, k));
                for x in &dep_items[j].as_ref().unwrap()[k].idents {
                    if seen_name.insert(x.clone()) {
                        work.push(x.clone());
                    }
                }
            }
        }
        reached.sort_unstable();
        if std::env::var_os("ZUSTC_DEBUG_REACH").is_some() {
            let mut cnt: FxMap<String, usize> = FxMap::default();
            for &(j, k) in &reached {
                let it = &dep_items[j].as_ref().unwrap()[k];
                *cnt.entry(it.names.first().cloned().unwrap_or_default()).or_default() += 1;
            }
            let mut v: Vec<(usize, String)> = cnt.into_iter().map(|(a, b)| (b, a)).collect();
            v.sort_unstable_by(|a, b| b.cmp(a));
            eprintln!("reach {}: {} items, top {:?}", c.name, reached.len(), &v[..v.len().min(12)]);
        }
        let mut h = KeyHasher::new();
        h.add(&self.cid);
        h.add(&(self.opts.opt as u8));
        // (the backend: a wasm32 crate's object is its module parts, an x86-64 one an ELF object)
        h.add(&self.opts.backend_id());
        h.add(&(c.lib as u8));
        add_crate_cfg(&mut h, c);
        h.add(own);
        for f in &own_map.files {
            h.add(f);
        }
        for &j in deps {
            h.add(&crates[j].name);
            for f in &loaded[j].as_ref().unwrap().1.files {
                h.add(f);
            }
        }
        for &(j, k) in &reached {
            h.add(&(j as u64));
            h.add(&dep_items[j].as_ref().unwrap()[k].hash);
        }
        (key_hex(h.finish()), reached)
    }

    /// Builds a workspace: makes a .o per crate in out_dir (not written if unchanged),
    /// and for each bin links its own .o and its dependency crates' .o. They share this Session2's cache
    pub fn build_workspace(&mut self, crates: &[CrateSpec], out_dir: &std::path::Path) -> R<WsReport> {
        // (wasm32: a crate's object is the parts of the module (write_parts); a bin's "link" writes the module from its
        // parts and its dependencies' with the module writer itself)
        let module_writer = self.opts.module_writer;
        if module_writer.is_some() {
            self.opts.module_writer = Some(write_parts);
        }
        let r = self.build_workspace_in(crates, out_dir, module_writer);
        self.opts.module_writer = module_writer;
        r
    }

    fn build_workspace_in(&mut self, crates: &[CrateSpec], out_dir: &std::path::Path, module_writer: Option<ModuleWriter>) -> R<WsReport> {
        let t0 = now_ns();
        // (a test crate is configured with `test`: in its fingerprint too)
        let with_test: Vec<CrateSpec>;
        let crates = if crates.iter().any(|c| c.test && !c.cfgs.iter().any(|x| x.0 == "test" && x.1.is_none())) {
            with_test = crates
                .iter()
                .map(|c| {
                    let mut c = c.clone();
                    if c.test {
                        c.cfgs.push(("test".to_string(), None));
                    }
                    c
                })
                .collect();
            &with_test[..]
        } else {
            crates
        };
        let order = topo(crates)?;
        mkdir_all(out_dir)?;
        // (the sources of the crates read: name → (the text made into `pub mod name { .. }`, the file map))
        let mut loaded: Vec<Option<(String, crate::srcmap::SrcMap)>> = vec![None; crates.len()];
        for &i in &order {
            let c = &crates[i];
            // (a workspace's crates have no edition of their own here (zurgo gives each package's to rustc only): the
            // edition-dependent token checks stay off, as an edition 2018 package may write `m!(foo#bar)`)
            let cfg = crate::cfg::CfgSet { edition: 0, ..self.opts.cfg_of(&c.features, &c.cfgs, &c.env) };
            let (text, map) = crate::srcmap::load_crate(c.root.as_path(), &c.display, &cfg)?;
            let text = if c.test { with_harness(&text, &c.display, &self.opts.cfg_of(&c.features, &c.cfgs, &c.env)) } else { text };
            let text = if c.lib { format!("pub mod {} {{ {text}\n}}\n", c.name) } else { text };
            loaded[i] = Some((text, map));
        }
        let mut rep = WsReport::default();
        let mut changed = vec![false; crates.len()];
        // (transitive dependencies: in dependency order)
        let all_deps = |i: usize| -> Vec<usize> {
            let mut seen = vec![false; crates.len()];
            let mut st: Vec<usize> = vec![i];
            while let Some(k) = st.pop() {
                for d in &crates[k].deps {
                    if let Some(j) = crates.iter().position(|c| &c.name == d) {
                        if !seen[j] {
                            seen[j] = true;
                            st.push(j);
                        }
                    }
                }
            }
            order.iter().copied().filter(|&j| seen[j]).collect()
        };
        // (the hash of the parts of a library that affect its users)
        // (if the source is the same as last time, read last time's from out_dir/<name>.iface: `source-hash interface-hash`)
        let mut ifaces: Vec<Key> = vec![0; crates.len()];
        for i in 0..crates.len() {
            if !crates[i].lib {
                continue;
            }
            let text = &loaded[i].as_ref().unwrap().0;
            let mut h = KeyHasher::new();
            h.add(&self.cid);
            h.add(text);
            let raw = key_hex(h.finish());
            let ip = out_dir.join(format!("{}.iface", crates[i].name).as_str());
            let old = std::fs::read_to_string(ip.as_path()).unwrap_or_default();
            let hit = match old.split_once(' ') {
                Some((r, v)) if r == raw && v.len() == 32 => parse_hex(v),
                _ => None,
            };
            ifaces[i] = match hit {
                Some(v) => v,
                None => {
                    let v = interface_hash(text);
                    let _ = std::fs::write(ip.as_path(), format!("{raw} {}", key_hex(v)));
                    v
                }
            };
        }
        // (each crate's features, cfgs and environment: add_crate_cfg)
        let cfg_keys: Vec<Key> = crates
            .iter()
            .map(|c| {
                let mut h = KeyHasher::new();
                add_crate_cfg(&mut h, c);
                h.finish()
            })
            .collect();
        // Stale crates: (index, fingerprint)
        let mut stale: Vec<(usize, String)> = vec![];
        for &i in &order {
            let c = &crates[i];
            let deps = all_deps(i);
            // The fingerprint: the compiler, the settings, own source, the dependency crates' interfaces. If it is the same as last time and the .o exists, do nothing
            let mut h = KeyHasher::new();
            h.add(&self.cid);
            h.add(&(self.opts.opt as u8));
            h.add(&self.opts.backend_id());
            // (the edition decides what closures capture, among others)
            h.add(&self.opts.edition);
            h.add(&(c.lib as u8));
            h.add(&cfg_keys[i]);
            h.add(&loaded[i].as_ref().unwrap().0);
            for f in &loaded[i].as_ref().unwrap().1.files {
                h.add(f);
            }
            for &j in &deps {
                h.add(&crates[j].name);
                h.add(&ifaces[j]);
                // (and the dependency's: what its generic code expands to)
                h.add(&cfg_keys[j]);
                for f in &loaded[j].as_ref().unwrap().1.files {
                    h.add(f);
                }
            }
            let fp = key_hex(h.finish());
            let fp_path = out_dir.join(format!("{}.fp", c.name).as_str());
            let op = out_dir.join(format!("{}.o", c.name).as_str());
            if !self.opts.no_cache && op.exists() && std::fs::read_to_string(fp_path.as_path()).is_ok_and(|old| old == fp) {
                if self.opts.time {
                    eprintln!("  {:<12} fresh", c.name);
                }
                rep.crates.push((c.name.clone(), Report2::default()));
                continue;
            }
            stale.push((i, fp));
        }
        // The sources a stale crate uses: for itself, its expansion with only the dependency crates' macros attached; for the dependency crates,
        // their expansion with the bodies of non-generic fns removed (stub::interface_text). Both are made only once per crate
        // (re-expanding and re-analyzing the whole source of the dependency crates on the user's side would, with many dependencies, scale with the size of the transitive dependencies)
        // (proc macros expand only here: with them, ZUSTC_NO_DEP_STUB does not turn it off)
        let reuse = std::env::var_os("ZUSTC_NO_DEP_STUB").is_none() || crates.iter().any(|c| c.proc_macro.is_some());
        let mut fp2s: Vec<(usize, String)> = vec![];
        // (for each stale crate, the items of the dependency crates' interfaces reachable by name: only those are passed to the user)
        let mut reach_of: Vec<Option<Vec<(usize, usize)>>> = vec![None; crates.len()];
        let mut dep_items_all: Vec<Option<Vec<crate::stub::StubItem>>> = (0..crates.len()).map(|_| None).collect();
        let mut own_v: Vec<Option<(String, crate::srcmap::SrcMap)>> = loaded.clone();
        let mut dep_v: Vec<Option<(String, crate::srcmap::SrcMap)>> = loaded.clone();
        if reuse && !stale.is_empty() {
            let t2 = now_ns();
            let mut need = vec![false; crates.len()];
            let mut need_dep = vec![false; crates.len()];
            for &(i, _) in &stale {
                need[i] = true;
                for j in all_deps(i) {
                    need[j] = true;
                    need_dep[j] = true;
                }
            }
            let mut pub_uses: Vec<Option<Vec<(String, String, String)>>> = vec![None; crates.len()];
            let defs: Vec<String> = (0..crates.len())
                .map(|i| if need[i] { crate::macros::macro_defs(&loaded[i].as_ref().unwrap().0) } else { String::new() })
                .collect();
            for &i in &order {
                if !need[i] {
                    continue;
                }
                let (text, map) = loaded[i].clone().unwrap();
                let mut dd = String::new();
                // (the proc-macro crates it reaches: directly, or through the libraries that re-export their macros)
                let mut pms: Vec<crate::pm::Lib> = vec![];
                for j in all_deps(i) {
                    dd.push_str(&defs[j]);
                    if let Some(p) = &crates[j].proc_macro {
                        pms.push(crate::pm::load(&crates[j].name, p).map_err(|e| format!("crate `{}`: the proc-macro crate `{}`: {e}", crates[i].name, crates[j].name))?);
                    }
                }
                // (which of their macros the libraries re-export, and under what names: `re::answer!()`. From their
                // expanded text, prepared before this crate's: a `pub use` a macro of theirs gives counts too)
                if !pms.is_empty() {
                    for j in all_deps(i) {
                        if crates[j].proc_macro.is_none() {
                            let uses = pub_uses[j].get_or_insert_with(|| crate::pm::pub_uses(&own_v[j].as_ref().or(loaded[j].as_ref()).unwrap().0));
                            crate::pm::note_reexports(&mut pms, &crates[j].name, uses);
                        }
                    }
                }
                // (with the same input as last time, read from out_dir/<name>.prep: `key\nlength of the expansion\nexpansion interface`)
                let mut kh = KeyHasher::new();
                kh.add(&self.cid);
                kh.add(&text);
                kh.add(&dd);
                // (the expansion's cfg follows the target: `cfg(target_arch = "wasm32")` in a macro's output)
                kh.add(&self.opts.backend_id());
                add_crate_cfg(&mut kh, &crates[i]);
                // (the proc macros' dylibs: also when one is rebuilt in place)
                for j in all_deps(i) {
                    if crates[j].proc_macro.is_some() {
                        add_crate_cfg(&mut kh, &crates[j]);
                    }
                }
                for l in &pms {
                    for (c, a, o) in &l.via {
                        kh.add(&format!("{}:{c}:{a}:{o}", l.name));
                    }
                }
                let pkey = key_hex(kh.finish());
                let pp = out_dir.join(format!("{}.prep", crates[i].name).as_str());
                // (`--no-cache`: expanded again, also a proc macro's output, which may read files of its own)
                if let Some(old) = (!self.opts.no_cache).then(|| std::fs::read_to_string(pp.as_path()).ok()).flatten() {
                    if let Some((k, rest)) = old.split_once('\n') {
                        if k == pkey {
                            if let Some((nl, body)) = rest.split_once('\n') {
                                if let Ok(nl) = nl.parse::<usize>() {
                                    if nl <= body.len() && body.is_char_boundary(nl) {
                                        // (a cached proc macro's output: as when it expanded here, the hoisting looks
                                        // for its marked lines)
                                        if !pms.is_empty() && body.contains(crate::lexer::PM_MARK) {
                                            crate::lexer::PM_EXPANDED.store(true, std::sync::atomic::Ordering::Relaxed);
                                        }
                                        dep_v[i] = Some((body[nl..].to_string(), map.clone()));
                                        own_v[i] = Some((body[..nl].to_string(), map));
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
                let marker = crate::macros::DEP_MACROS_MARKER;
                let exp = if dd.is_empty() && !text.contains("macro_rules!") && pms.is_empty() {
                    text.clone()
                } else {
                    let mut full = text.clone();
                    full.push_str(marker);
                    full.push_str(&dd);
                    let cfg = self.opts.cfg_of(&crates[i].features, &crates[i].cfgs, &crates[i].env);
                    match crate::macros::expand_with(&full, Some(&cfg), &pms).map_err(|e| format!("crate `{}`: {}", crates[i].name, map.relocate_err(&e)))? {
                        Some(t) => match t.find(marker) {
                            Some(k) => t[..k].to_string(),
                            None => t,
                        },
                        None => text.clone(),
                    }
                };
                let stub = crate::stub::interface_text(&exp);
                if !self.opts.no_cache {
                    let _ = std::fs::write(pp.as_path(), format!("{pkey}\n{}\n{exp}{stub}", exp.len()));
                }
                let _ = need_dep[i];
                dep_v[i] = Some((stub, map.clone()));
                own_v[i] = Some((exp, map));
            }
            if self.opts.time {
                eprintln!("  {:<12} {:>8.2} ms", "(prepare)", (now_ns() - t2) as f64 / 1e6);
            }
            // The precise fingerprint: own (expanded) source, and the items of the dependency crates' interfaces reachable by name from it.
            // If it is the same as last time, do not rebuild (only unused parts of the dependency crates changed: since the bodies are removed, the signatures of the fns of
            // dependencies' dependencies have no effect unless their names are reached from, e.g., the bodies of the generic fns calling them)
            let t3 = now_ns();
            let mut dep_items: Vec<Option<Vec<crate::stub::StubItem>>> = (0..crates.len()).map(|_| None).collect();
            let mut keep: Vec<(usize, String)> = vec![];
            for (i, fp) in std::mem::take(&mut stale) {
                let deps = all_deps(i);
                for &j in &deps {
                    if dep_items[j].is_none() {
                        dep_items[j] = Some(crate::stub::items(&dep_v[j].as_ref().unwrap().0));
                    }
                }
                let own = &own_v[i].as_ref().unwrap().0;
                let (fp2, reached) = self.precise_fp(&crates[i], own, &loaded[i].as_ref().unwrap().1, &deps, crates, &loaded, &dep_items);
                let p2 = out_dir.join(format!("{}.fp2", crates[i].name).as_str());
                let op = out_dir.join(format!("{}.o", crates[i].name).as_str());
                if !self.opts.no_cache && op.exists() && std::fs::read_to_string(p2.as_path()).is_ok_and(|old| old == fp2) {
                    let _ = std::fs::write(out_dir.join(format!("{}.fp", crates[i].name).as_str()), &fp);
                    if self.opts.time {
                        eprintln!("  {:<12} fresh (used interface unchanged)", crates[i].name);
                    }
                    rep.crates.push((crates[i].name.clone(), Report2::default()));
                    continue;
                }
                fp2s.push((i, fp2));
                reach_of[i] = Some(reached);
                keep.push((i, fp));
            }
            stale = keep;
            dep_items_all = dep_items;
            if self.opts.time {
                eprintln!("  {:<12} {:>8.2} ms", "(fp2)", (now_ns() - t3) as f64 / 1e6);
            }
        }
        // (crates use only each other's sources (not the .o), so the stale ones are forked and compiled concurrently)
        let par = self.opts.jobs.min(stale.len()).max(1);
        let mut results: Vec<Option<R<(Report2, bool, u64)>>> = vec![None; stale.len()];
        if par > 1 {
            let inner = (self.opts.jobs / par).max(1);
            let mut k0 = 0;
            while k0 < stale.len() {
                let k1 = (k0 + par).min(stale.len());
                let mut kids: Vec<(usize, i32, i32)> = vec![];
                for k in k0..k1 {
                    let mut fds = [0i32; 2];
                    // SAFETY: fds is 2 ints
                    if unsafe { c_pipe(fds.as_mut_ptr()) } != 0 {
                        continue;
                    }
                    let pid = fork_counted();
                    if pid == 0 {
                        unsafe { c_close(fds[0]) };
                        self.opts.jobs = inner;
                        let (i, fp) = (stale[k].0, stale[k].1.clone());
                        self.fresh.clear();
                        let r = self.build_one(crates, &own_v, &dep_v, (&dep_items_all, match &reach_of[i] { Some(v) => Some(v.as_slice()), None => None }), i, &all_deps(i), &fp, out_dir);
                        let mut buf = vec![];
                        match r {
                            Ok((rp, ch, ns)) => {
                                buf.push(0);
                                buf.push(ch as u8);
                                for v in [rp.fns_compiled as u64, rp.fns_hit as u64, rp.items_parsed as u64, rp.object_bytes as u64, ns] {
                                    crate::cache2::put_uv(&mut buf, v);
                                }
                                // (also return the cache entries made (summaries, fns' machine code): crates in later batches inherit them through fork and use them.
                                //  the prelude's monomorphizations and the summaries of the dependency crates' items are the same across many crates)
                                for &k in &self.fresh {
                                    if let Some(v) = self.mem.get(&k) {
                                        buf.extend_from_slice(&(k as u64).to_le_bytes());
                                        buf.extend_from_slice(&((k >> 64) as u64).to_le_bytes());
                                        crate::cache2::put_uv(&mut buf, v.len() as u64);
                                        buf.extend_from_slice(v);
                                    }
                                }
                            }
                            Err(e) => {
                                buf.push(1);
                                buf.extend_from_slice(e.as_bytes());
                            }
                        }
                        write_all(fds[1], &buf);
                        unsafe { c_close(fds[1]) };
                        unsafe { c_exit(0) };
                    }
                    unsafe { c_close(fds[1]) };
                    if pid < 0 {
                        unsafe { c_close(fds[0]) };
                        continue;
                    }
                    kids.push((k, pid, fds[0]));
                }
                for (k, pid, fd) in kids {
                    let d = read_all(fd);
                    unsafe { c_close(fd) };
                    let mut st = 0i32;
                    unsafe { c_waitpid(pid, &mut st, 0) };
                    let mut r = crate::cache2::Rd { d: &d, pos: 0 };
                    results[k] = match r.u8() {
                        Some(0) => {
                            let ch = r.u8().unwrap_or(1) != 0;
                            let mut v = [0u64; 5];
                            for x in v.iter_mut() {
                                *x = r.uv().unwrap_or(0);
                            }
                            let rp = Report2 { fns_compiled: v[0] as usize, fns_hit: v[1] as usize, items_parsed: v[2] as usize, object_bytes: v[3] as usize, linked: false };
                            while r.pos < d.len() {
                                let (Some(lo), Some(hi), Some(n)) = (r.u64(), r.u64(), r.uv()) else { break };
                                let Some(body) = r.take(n as usize) else { break };
                                let k = (lo as u128) | ((hi as u128) << 64);
                                if !self.mem.contains_key(&k) {
                                    self.mem.insert(k, body.to_vec());
                                }
                            }
                            Some(Ok((rp, ch, v[4])))
                        }
                        Some(_) => Some(Err(String::from_utf8_lossy(&d[1..]).into_owned())),
                        // (the child crashed: once more here (if it is an error, that comes out))
                        None => None,
                    };
                }
                k0 = k1;
            }
        }
        for k in 0..stale.len() {
            let (i, fp) = (stale[k].0, stale[k].1.clone());
            let r = match results[k].take() {
                Some(r) => r,
                None => self.build_one(crates, &own_v, &dep_v, (&dep_items_all, match &reach_of[i] { Some(v) => Some(v.as_slice()), None => None }), i, &all_deps(i), &fp, out_dir),
            };
            let (r, ch, ns) = r?;
            if !self.opts.no_cache {
                if let Some((_, f2)) = fp2s.iter().find(|x| x.0 == i) {
                    let _ = std::fs::write(out_dir.join(format!("{}.fp2", crates[i].name).as_str()), f2);
                }
            }
            changed[i] = ch;
            if ch {
                rep.objects_written += 1;
            }
            if self.opts.time {
                eprintln!("  {:<12} {:>8.2} ms  {} fns compiled, {} hit, {} items parsed{}", crates[i].name, ns as f64 / 1e6, r.fns_compiled, r.fns_hit, r.items_parsed, if ch { "" } else { " (object unchanged)" });
            }
            rep.crates.push((crates[i].name.clone(), r));
        }
        for &i in &order {
            let c = &crates[i];
            if c.lib {
                continue;
            }
            let deps = all_deps(i);
            let exe_name = c.exe.as_deref().unwrap_or(c.name.as_str());
            let exe = match module_writer {
                Some(_) => out_dir.join(format!("{exe_name}.wasm").as_str()),
                None => out_dir.join(exe_name),
            };
            // (what this executable is linked from: the crates' names, fingerprints and objects' sizes and times, kept
            //  next to it. An unchanged object is not enough when another bin crate wrote this executable last (`exe=`
            //  moved between crates), and an object written again is linked again (a `--no-cache` build has no
            //  fingerprints, also one that stopped before its links))
            let mut stamp = String::new();
            for j in std::iter::once(i).chain(deps.iter().copied()) {
                stamp.push_str(&crates[j].name);
                stamp.push(' ');
                stamp.push_str(&std::fs::read_to_string(out_dir.join(format!("{}.fp", crates[j].name).as_str())).unwrap_or_default());
                if let Ok(m) = std::fs::metadata(out_dir.join(format!("{}.o", crates[j].name).as_str())) {
                    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
                    stamp.push_str(&format!(" {} {t}", m.len()));
                }
                stamp.push('\n');
            }
            let stamp_path = out_dir.join(format!("{exe_name}.linked").as_str());
            if !changed[i] && !deps.iter().any(|&j| changed[j]) && exe.exists() && std::fs::read_to_string(stamp_path.as_path()).is_ok_and(|s| s == stamp) {
                continue;
            }
            // (no stamp while linking: a link that fails is tried again, also by a `--no-cache` build that leaves the
            //  fingerprints as they were)
            let _ = std::fs::remove_file(stamp_path.as_path());
            let t1 = now_ns();
            let mut objs: Vec<std::path::PathBuf> = vec![out_dir.join(format!("{}.o", c.name).as_str())];
            objs.extend(deps.iter().map(|&j| out_dir.join(format!("{}.o", crates[j].name).as_str())));
            match module_writer {
                Some(w) => {
                    let mut parts = vec![];
                    for o in &objs {
                        let d = std::fs::read(o).map_err(|e| format!("{}: {e}", o.display()))?;
                        parts.push(read_parts(&d).ok_or_else(|| format!("{}: not the parts of a wasm32 module (built for another target?)", o.display()))?);
                    }
                    let (funcs, datas) = merge_parts(parts)?;
                    let m = w(&funcs, &datas)?;
                    std::fs::write(exe.as_path(), m).map_err(|e| format!("{}: {e}", exe.display()))?;
                }
                None => {
                    let refs: Vec<&std::path::Path> = objs.iter().map(|p| p.as_path()).collect();
                    crate::link::link_many(self.opts.linker.as_deref(), &refs, exe.as_path())?;
                }
            }
            let _ = std::fs::write(stamp_path.as_path(), &stamp);
            rep.linked += 1;
            if self.opts.time {
                eprintln!("  {:<12} {:>8.2} ms  link {} objects", format!("link {}", c.name), (now_ns() - t1) as f64 / 1e6, objs.len());
            }
        }
        self.ctx = CrateCtx::default();
        if self.opts.time {
            eprintln!("  {:<12} {:>8.2} ms", "total", (now_ns() - t0) as f64 / 1e6);
        }
        Ok(rep)
    }
}


/// The functions a static's initializer names as values (fn pointers, also inside arrays, tuples and structs)
fn static_fns(prog: &crate::program::Program, e: &crate::parse::Expr, out: &mut Vec<u32>, depth: u32) {
    use crate::parse::Expr;
    if depth > 32 {
        return;
    }
    if let Some(f) = crate::ast_consteval::fn_of(prog, e) {
        out.push(f);
        return;
    }
    match e {
        Expr::Paren(x) | Expr::Ref(_, x) | Expr::Cast(x, _) => static_fns(prog, x, out, depth + 1),
        // (a block of only its tail (`{ f }`), as the constant evaluation reads it)
        Expr::Block(None, _, b) if b.stmts.len() == 1 => {
            if let crate::parse::Stmt::Expr(x, false) = &b.stmts[0] {
                static_fns(prog, x, out, depth + 1);
            }
        }
        Expr::Array(xs) | Expr::Tuple(xs) | Expr::Call(_, xs) => xs.iter().for_each(|x| static_fns(prog, x, out, depth + 1)),
        Expr::Struct(_, fs, _) => fs.iter().for_each(|f| static_fns(prog, &f.1, out, depth + 1)),
        _ => {}
    }
}

/// The native libraries of the program's `#[link(name = "x")]` extern blocks: `lib<x>.a` in a `-L` directory, else
/// `-lx` (passed to the linker as is). Only items that start with the attribute are read
fn native_libs(src: &str, items: &[SrcItem], dirs: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = vec![];
    for it in items {
        let Some(t) = src.get(it.start..it.end) else { continue };
        let mut rest = t;
        while let Some(r) = rest.trim_start().strip_prefix("#[") {
            let Some(e) = r.find(']') else { break };
            let attr = &r[..e];
            rest = &r[e + 1..];
            let Some(args) = attr.trim().strip_prefix("link").map(str::trim_start).and_then(|a| a.strip_prefix('(')) else { continue };
            let Some(n) = args.find("name") else { continue };
            let v = args[n + 4..].trim_start();
            let Some(v) = v.strip_prefix('=').map(str::trim_start).and_then(|v| v.strip_prefix('"')) else { continue };
            let Some(q) = v.find('"') else { continue };
            let name = &v[..q];
            let a = format!("lib{name}.a");
            let lib = dirs.iter().map(|d| d.join(&a)).find(|p| p.is_file()).unwrap_or_else(|| format!("-l{name}").into());
            if !out.contains(&lib) {
                out.push(lib);
            }
        }
    }
    out
}
