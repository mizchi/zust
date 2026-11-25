//! `import { greet, Shape } from "./lib.zsc";`: the types and functions of another zustscript module.
//!
//! Each module is compiled on its own, as TS compiles each file: the other module is read, checked, and what is
//! imported comes in as
//! - its records and enums, each marked as coming from that module (`from`), so the `.d.ts` imports them
//!   (`import type { Shape } from "./lib.js"`) instead of declaring them again. The types they use come in too;
//! - its `export fn`s, as `extern fn`s of the module `./lib.js` (the JS that zsc writes for it): the JS output imports
//!   them, and the checker sees their signatures.
//!
//! Not yet: a function with a numeric bound (one JS function per numeric type), one that performs an effect (a
//! generator), a raw Rust `wasm fn`, and calling an imported function from an `@wasm fn` (an `extern` there is an
//! error already). The specifier is the file's with `.zsc` replaced by `.js`: the modules are compiled into one
//! directory tree that mirrors the sources.

use crate::ast::{Item, Member, Pos, TsFrom, TyS};
use crate::tsimport::TsError;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Replaces each `import { .. } from "./x.zsc"` (and `import type` from one) by what it names. `stack`: the files
/// being read, to report a cycle.
pub fn expand(items: Vec<Item>, base: &Path, stack: &mut Vec<PathBuf>) -> Result<Vec<Item>, Vec<TsError>> {
    expand_in(items, base, stack, &mut HashMap::new())
}

/// (`modules`: every module read in this compilation, by canonical path: one reached through two importers is
/// read and checked once)
fn expand_in(items: Vec<Item>, base: &Path, stack: &mut Vec<PathBuf>, modules: &mut HashMap<PathBuf, Rc<Module>>) -> Result<Vec<Item>, Vec<TsError>> {
    let mut out = vec![];
    let mut errs = vec![];
    // (a type brought in once, even when two imports need it: by the file that defines it and its name)
    let mut done: BTreeSet<(PathBuf, String)> = BTreeSet::new();
    // (a bare input file name has an empty base: the current directory)
    let base = if base.as_os_str().is_empty() { Path::new(".") } else { base };
    let here = std::fs::canonicalize(base).unwrap_or(base.to_path_buf());
    for it in items {
        let Item::ImportType { names, from, values, pos } = &it else {
            out.push(it);
            continue;
        };
        if !from.ends_with(".zsc") {
            out.push(it);
            continue;
        }
        if !from.starts_with("./") && !from.starts_with("../") {
            errs.push(TsError { file: None, pos: *pos, msg: format!("`{from}`: a module is imported by a relative path (`./lib.zsc`)") });
            continue;
        }
        let path = base.join(from);
        let key = std::fs::canonicalize(&path).unwrap_or(path.clone());
        if stack.contains(&key) {
            errs.push(TsError { file: None, pos: *pos, msg: format!("`{from}`: an import cycle (a module cannot import one that imports it)") });
            continue;
        }
        if !modules.contains_key(&key) {
            match Module::read(&path, &key, stack, modules) {
                Ok(m) => {
                    modules.insert(key.clone(), Rc::new(m));
                }
                Err(mut e) => {
                    if e.is_empty() {
                        e.push(TsError { file: None, pos: *pos, msg: format!("{}: cannot be read", path.display()) });
                    }
                    // (an error that has no file of its own is the import's)
                    for x in &mut e {
                        if x.file.is_none() {
                            x.pos = *pos;
                        }
                    }
                    errs.extend(e);
                    continue;
                }
            }
        }
        let m = modules[&key].clone();
        let spec = format!("{}.js", from.strip_suffix(".zsc").unwrap());
        let mut types: Vec<String> = vec![];
        for (n, npos) in names {
            match m.find(n) {
                Some(Item::Record { .. }) | Some(Item::Enum { .. }) | Some(Item::ExternType { .. }) => types.push(n.clone()),
                Some(Item::Fn { public: false, .. }) | Some(Item::WasmFn { public: false, .. }) => {
                    errs.push(TsError { file: None, pos: *npos, msg: format!("`{from}` does not export `{n}` (write `export fn {n}` there)") });
                }
                Some(Item::Fn { .. }) | Some(Item::WasmFn { .. }) if !values => {
                    errs.push(TsError { file: None, pos: *npos, msg: format!("`{n}` is a function: `import {{ {n} }} from \"{from}\";` (`import type` brings types only)") });
                }
                Some(Item::Fn { bounds, .. }) if !bounds.is_empty() => {
                    errs.push(TsError { file: None, pos: *npos, msg: format!("`{n}` has a numeric bound (one JS function per numeric type): it cannot be imported yet") });
                }
                Some(Item::Fn { name, .. }) if m.effects.get(name).is_some_and(|e| !e.is_empty()) => {
                    errs.push(TsError { file: None, pos: *npos, msg: format!("`{n}` performs an effect (a generator in JS): it cannot be imported yet") });
                }
                Some(Item::Fn { name, generics, args, ret, .. }) => {
                    let args: Vec<TyS> = args.iter().map(|(_, t)| t.clone()).collect();
                    let ret = ret.clone().unwrap_or(TyS::Unit);
                    for t in args.iter().chain([&ret]) {
                        uses(t, generics, &mut types);
                    }
                    out.push(Item::Extern { name: name.clone(), module: Some(spec.clone()), generics: generics.clone(), args, ret, js: crate::js::ident(name), pos: *npos });
                }
                Some(Item::WasmFn { .. }) => {
                    errs.push(TsError { file: None, pos: *npos, msg: format!("`{n}` is a Rust `wasm fn`: it cannot be imported yet (an `@wasm fn` can)") });
                }
                _ => errs.push(TsError { file: None, pos: *npos, msg: format!("`{from}` declares no type or function `{n}`") }),
            }
        }
        // the types, and the types they use (a record's fields, an enum's variants), whatever their names look
        // like: the module's declarations say which names are types
        while let Some(n) = types.pop() {
            let Some(it) = m.find(&n) else { continue };
            // (the prelude's `Option` / `Result`: the importing module has its own, the same type)
            if let Item::Enum { pos, .. } = it
                && crate::ast::is_prelude(*pos)
            {
                continue;
            }
            // (an extern type is JS's: the same declaration here, and the types its members use)
            if let Item::ExternType { params, members, .. } = it {
                if done.insert((key.clone(), n.clone())) {
                    for m in members {
                        match m {
                            Member::Prop { ty, .. } => uses(ty, params, &mut types),
                            Member::Method { args, ret, .. } => args.iter().chain([ret]).for_each(|t| uses(t, params, &mut types)),
                        }
                    }
                    out.push(it.clone());
                }
                continue;
            }
            let mut it = it.clone();
            let (Item::Record { from: f, .. } | Item::Enum { from: f, .. }) = &mut it else { continue };
            // where the type is defined: this module, or the module or TS file it came from
            let defined = match f {
                Some(TsFrom { zsc: Some(z), .. }) => z.clone(),
                Some(t) => t.path.clone(),
                None => key.clone(),
            };
            if !done.insert((defined.clone(), n.clone())) {
                continue;
            }
            match f {
                // (a TS file's type stays that file's: its path is canonical)
                Some(TsFrom { zsc: None, .. }) => {}
                // (a zustscript module's: its JS from here, which differs from the specifier the importing module
                // wrote when that module is in another directory)
                _ => *f = Some(from_module(&here, &defined)),
            }
            match &it {
                Item::Record { params, fields, .. } => {
                    for (_, t) in fields {
                        uses(t, params, &mut types);
                    }
                }
                Item::Enum { params, variants, .. } => {
                    for (_, ts, r) in variants {
                        for t in ts.iter().chain(r.iter()) {
                            uses(t, params, &mut types);
                        }
                    }
                }
                _ => {}
            }
            out.push(it);
        }
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

/// The mark of a type of the zustscript module `file`, used from the directory `here`: exported there (every type
/// is, in its `.d.ts`), in that module's JS shape (`repr: None`), and imported from its JS by a relative specifier
fn from_module(here: &Path, file: &Path) -> TsFrom {
    let a: Vec<_> = here.components().collect();
    let b: Vec<_> = file.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); a.len() - common];
    parts.extend(b[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    let rel = parts.join("/");
    let rel = if rel.starts_with("../") { rel } else { format!("./{rel}") };
    let spec = format!("{}.js", rel.strip_suffix(".zsc").unwrap_or(&rel));
    TsFrom { path: PathBuf::from(spec), exported: true, members: vec![], text: String::new(), repr: None, zsc: Some(file.to_path_buf()) }
}

/// The named types `t` uses (not its generic parameters, not the built-in ones)
fn uses(t: &TyS, params: &[String], out: &mut Vec<String>) {
    match t {
        TyS::Name(n, args, _) => {
            if !params.contains(n) {
                out.push(n.clone());
            }
            for a in args {
                uses(a, params, out);
            }
        }
        TyS::Tuple(ts) => ts.iter().for_each(|t| uses(t, params, out)),
        TyS::Fn(a, r) => {
            a.iter().for_each(|t| uses(t, params, out));
            uses(r, params, out);
        }
        TyS::Unit => {}
    }
}

struct Module {
    items: Vec<Item>,
    effects: HashMap<String, BTreeSet<String>>,
}

impl Module {
    /// Reads, expands and checks another module: an error in it is reported in its own file
    fn read(path: &Path, key: &Path, stack: &mut Vec<PathBuf>, modules: &mut HashMap<PathBuf, Rc<Module>>) -> Result<Module, Vec<TsError>> {
        let src = std::fs::read_to_string(path).map_err(|e| vec![TsError { file: None, pos: Pos { line: 1, col: 0 }, msg: format!("{}: {e}", path.display()) }])?;
        let file = path.display().to_string();
        let at = |pos: Pos, msg: String| TsError { file: Some((file.clone(), src.clone())), pos, msg };
        let items = crate::parse::Parser::new(&src).file().map_err(|(p, m)| vec![at(p, m)])?;
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        stack.push(key.to_path_buf());
        let items = expand_in(items, &dir, stack, modules);
        stack.pop();
        let items = items.map_err(|es| es.into_iter().map(|e| if e.file.is_some() { e } else { at(e.pos, e.msg) }).collect::<Vec<_>>())?;
        let items = crate::tsimport::expand(items, &dir).map_err(|es| es.into_iter().map(|e| if e.file.is_some() { e } else { at(e.pos, e.msg) }).collect::<Vec<_>>())?;
        let items = crate::ast::with_prelude(items);
        let mut ck = crate::check::Checker::new();
        ck.check_file(&items, false);
        if !ck.errors.is_empty() {
            let mut es = ck.errors.clone();
            es.sort_by_key(|e| (e.0.line, e.0.col));
            es.dedup();
            return Err(es.into_iter().map(|(p, m)| at(p, m)).collect());
        }
        Ok(Module { items, effects: ck.info.effects })
    }

    fn find(&self, n: &str) -> Option<&Item> {
        self.items.iter().find(|it| match it {
            Item::Record { name, .. } | Item::Enum { name, .. } | Item::Fn { name, .. } | Item::WasmFn { name, .. } | Item::ExternType { name, .. } => name == n,
            _ => false,
        })
    }
}
