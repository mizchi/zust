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

use crate::ast::{Item, Pos, TsFrom, TyS};
use crate::tsimport::TsError;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Replaces each `import { .. } from "./x.zsc"` (and `import type` from one) by what it names. `stack`: the files
/// being read, to report a cycle.
pub fn expand(items: Vec<Item>, base: &Path, stack: &mut Vec<PathBuf>) -> Result<Vec<Item>, Vec<TsError>> {
    let mut out = vec![];
    let mut errs = vec![];
    let mut modules: HashMap<PathBuf, Module> = HashMap::new();
    // (a type brought in once, even when two imports need it)
    let mut done: BTreeSet<(PathBuf, String)> = BTreeSet::new();
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
            match Module::read(&path, &key, stack) {
                Ok(m) => {
                    modules.insert(key.clone(), m);
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
        let m = &modules[&key];
        let spec = format!("{}.js", from.strip_suffix(".zsc").unwrap());
        let mut types: Vec<String> = vec![];
        for (n, npos) in names {
            match m.find(n) {
                Some(Item::Record { .. }) | Some(Item::Enum { .. }) => types.push(n.clone()),
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
        // the types, and the types they use (a record's fields, an enum's variants)
        while let Some(n) = types.pop() {
            if !done.insert((key.clone(), n.clone())) {
                continue;
            }
            let Some(it) = m.find(&n) else { continue };
            let mut it = it.clone();
            match &mut it {
                Item::Record { params, fields, from: f, .. } => {
                    for (_, t) in fields.iter() {
                        uses(t, params, &mut types);
                    }
                    // (a record the other module read from TS stays that TS file's)
                    f.get_or_insert_with(|| from_module(&spec));
                }
                Item::Enum { params, variants, from: f, .. } => {
                    for (_, ts, r) in variants.iter() {
                        for t in ts.iter().chain(r.iter()) {
                            uses(t, params, &mut types);
                        }
                    }
                    f.get_or_insert_with(|| from_module(&spec));
                }
                _ => continue,
            }
            out.push(it);
        }
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

/// The mark of a type from another zustscript module: exported there (every type is, in its `.d.ts`), in that
/// module's JS shape (`repr: None`). The path is the specifier itself (relative, not a TS file's canonical path).
fn from_module(spec: &str) -> TsFrom {
    TsFrom { path: PathBuf::from(spec), exported: true, members: vec![], text: String::new(), repr: None }
}

/// The named types `t` uses (not its generic parameters, not the built-in ones)
fn uses(t: &TyS, params: &[String], out: &mut Vec<String>) {
    match t {
        TyS::Name(n, args, _) => {
            if !params.contains(n) && n.chars().next().is_some_and(|c| c.is_uppercase()) {
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
    fn read(path: &Path, key: &Path, stack: &mut Vec<PathBuf>) -> Result<Module, Vec<TsError>> {
        let src = std::fs::read_to_string(path).map_err(|e| vec![TsError { file: None, pos: Pos { line: 1, col: 0 }, msg: format!("{}: {e}", path.display()) }])?;
        let file = path.display().to_string();
        let at = |pos: Pos, msg: String| TsError { file: Some((file.clone(), src.clone())), pos, msg };
        let items = crate::parse::Parser::new(&src).file().map_err(|(p, m)| vec![at(p, m)])?;
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        stack.push(key.to_path_buf());
        let items = expand(items, &dir, stack);
        stack.pop();
        let items = items.map_err(|es| es.into_iter().map(|e| if e.file.is_some() { e } else { at(e.pos, e.msg) }).collect::<Vec<_>>())?;
        let items = crate::tsimport::expand(items, &dir).map_err(|es| es.into_iter().map(|e| if e.file.is_some() { e } else { at(e.pos, e.msg) }).collect::<Vec<_>>())?;
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
            Item::Record { name, .. } | Item::Enum { name, .. } | Item::Fn { name, .. } | Item::WasmFn { name, .. } => name == n,
            _ => false,
        })
    }
}
