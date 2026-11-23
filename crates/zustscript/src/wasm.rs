//! The wasm layer: the `wasm fn`s are Rust, compiled by zustc (`--target wasm32-unknown-unknown`). This module
//! writes that Rust file (the user's functions as written, plus an `extern "C"` wrapper per function that
//! lowers the boundary types), and the JS glue that calls the wrappers.
//!
//! The boundary, decided by the Rust types of the signature (no annotations, no `wasm_bindgen` macro):
//!
//! | Rust (wasm side) | JS layer | how it crosses |
//! |---|---|---|
//! | i32 / u32 / f64 / bool | i32 / u32 / f64 / bool | as wasm values |
//! | i64 | i64 (BigInt) | as a wasm i64 |
//! | `&[f64]` / `&[i32]` | `Array<f64>` / `Array<i32>` | copied into linear memory for the call, freed after |
//! | `&str` | String | UTF-16 → UTF-8 into linear memory for the call, freed after |
//! | `String` / `Vec<f64>` / `Vec<i32>` (result) | String / Array | the wrapper leaks it and writes (ptr, len, cap); the glue copies it out and frees it |
//! | `&dyn Fn(A) -> R` | `(A) => R` | a borrowed callback: registered for the call, released when the call returns |
//! | `Box<dyn Fn(A) -> R>` | `(A) => R` | an owned callback: released when the Rust side drops the Box |
//! | a record holding strings or arrays, an enum that does not fit the slots (or read from TS), an array of records / strings / arrays (`@wasm fn` only) | the same | as bytes in the format of the helpers: `encode` (npm `zustscript-helper`) and `from_bytes` (crate `zustscript_helper`, embedded), and back |
//!
//! Pointers and `usize` are 64-bit in zustc's wasm output (x86-64 layouts), so they cross as BigInt.

use crate::ast::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// The Rust side of the helpers (the crate zustscript_helper), embedded as `mod zustscript_helper`: zustc compiles
/// one file
const HELPER_RS: &str = include_str!("../helper/rust/src/lib.rs");

/// The JS spelling of a schema (helper/ts/index.js)
fn ws_js(w: &WS) -> String {
    match w {
        WS::Scalar(t) => format!("{:?}", rs_scalar(t)),
        WS::Str => "\"str\"".into(),
        WS::Arr(e) => format!("[\"arr\", {}]", ws_js(e)),
        WS::Rec(_, fs) => format!("[\"rec\", [{}]]", fs.iter().map(|(f, t)| format!("[{f:?}, {}]", ws_js(t))).collect::<Vec<_>>().join(", ")),
        WS::Enum { key, variants, .. } => {
            let k = key.as_ref().map_or_else(|| "null".to_string(), |k| format!("{k:?}"));
            let vs: Vec<String> = variants
                .iter()
                .map(|(_, tag, fs)| format!("[{tag:?}, [{}]]", fs.iter().map(|(f, t)| format!("[{f:?}, {}]", ws_js(t))).collect::<Vec<_>>().join(", ")))
                .collect();
            format!("[\"enum\", {k}, [{}]]", vs.join(", "))
        }
    }
}

/// The records and enums of a schema (each needs a `Wire` impl), by Rust type
fn ws_records<'w>(w: &'w WS, out: &mut BTreeMap<String, &'w WS>) {
    match w {
        WS::Arr(e) => ws_records(e, out),
        WS::Rec(rt, fs) => {
            out.insert(rt.clone(), w);
            for (_, t) in fs {
                ws_records(t, out);
            }
        }
        WS::Enum { rt, variants, .. } => {
            out.insert(rt.clone(), w);
            for (_, _, fs) in variants {
                for (_, t) in fs {
                    ws_records(t, out);
                }
            }
        }
        _ => {}
    }
}

/// The `Wire` impl of a record or enum of a schema
fn wire_impl(w: &WS, out: &mut String) {
    match w {
        WS::Rec(rt, fs) => {
            let puts: Vec<String> = fs.iter().map(|(f, _)| format!("        zustscript_helper::Wire::put(&self.{}, out);", crate::lower::local(f))).collect();
            let gets: Vec<String> = fs.iter().map(|(f, _)| format!("{}: zustscript_helper::Wire::get(r)", crate::lower::local(f))).collect();
            let _ = writeln!(
                out,
                "impl zustscript_helper::Wire for {rt} {{\n    fn put(&self, out: &mut Vec<u8>) {{\n{}\n    }}\n    fn get(r: &mut zustscript_helper::Reader) -> {rt} {{\n        {} {{ {} }}\n    }}\n}}\n",
                puts.join("\n"),
                base(rt),
                gets.join(", ")
            );
        }
        // (the variant's index, then its fields)
        WS::Enum { rt, variants, .. } => {
            let b = base(rt);
            let mut puts = String::new();
            let mut gets = String::new();
            for (k, (v, _, fs)) in variants.iter().enumerate() {
                let binds: Vec<String> = (0..fs.len()).map(|i| format!("__f{i}")).collect();
                let pat = if fs.is_empty() { format!("{b}::{v}") } else { format!("{b}::{v}({})", binds.join(", ")) };
                let _ = write!(puts, "            {pat} => {{\n                zustscript_helper::Wire::put(&{k}u32, out);\n");
                for x in &binds {
                    let _ = writeln!(puts, "                zustscript_helper::Wire::put({x}, out);");
                }
                puts.push_str("            }\n");
                let build = if fs.is_empty() { format!("{b}::{v}") } else { format!("{b}::{v}({})", vec!["zustscript_helper::Wire::get(r)"; fs.len()].join(", ")) };
                let _ = writeln!(gets, "            {k} => {build},");
            }
            let _ = writeln!(
                out,
                "impl zustscript_helper::Wire for {rt} {{\n    fn put(&self, out: &mut Vec<u8>) {{\n        match self {{\n{puts}        }}\n    }}\n    fn get(r: &mut zustscript_helper::Reader) -> {rt} {{\n        match <u32 as zustscript_helper::Wire>::get(r) {{\n{gets}            _ => panic!(\"zustscript: not a variant\"),\n        }}\n    }}\n}}\n"
            );
        }
        _ => {}
    }
}

fn rs_scalar(w: &WTy) -> &'static str {
    match w {
        WTy::I32 => "i32",
        WTy::U32 => "u32",
        WTy::I64 => "i64",
        WTy::F32 => "f32",
        WTy::F64 => "f64",
        WTy::Bool => "bool",
        WTy::Unit => "()",
        _ => unreachable!(),
    }
}

fn sig_key(args: &[WTy], ret: &WTy) -> String {
    let mut s: Vec<&str> = args.iter().map(rs_scalar).collect();
    s.push(match ret {
        WTy::Unit => "unit",
        r => rs_scalar(r),
    });
    s.join("_")
}

pub struct Wasm {
    /// the Rust source for zustc
    pub rust: String,
    /// the JS glue (goes into the module after the JS layer)
    pub glue: String,
    /// names the glue defines (the JS layer calls them)
    pub names: Vec<String>,
}

/// A function of the wasm layer: written in Rust (`wasm fn`), or lowered from zustscript (`@wasm fn`, see
/// lower.rs)
pub struct WFn {
    /// the JS name of its glue (an instance's for a numeric-bound function: `sum$f64`)
    pub js: String,
    /// the Rust function the wrapper calls
    pub rs: String,
    pub args: Vec<(String, WTy)>,
    pub ret: WTy,
    /// the Rust items (the function, and for `@wasm` what it was lowered to)
    pub text: String,
    /// exported from the JS module
    pub public: bool,
}

/// `raw`: Rust that needs no glue (lowered functions only other lowered functions call)
pub fn generate(items: &[Item], lowered: Vec<WFn>, raw: &str, wasm_file: &str) -> Option<Wasm> {
    let mut wfns: Vec<WFn> = items
        .iter()
        .filter_map(|it| match it {
            Item::WasmFn { name, args, ret, text, public, .. } => {
                Some(WFn { js: name.clone(), rs: name.clone(), args: args.clone(), ret: ret.clone(), text: text.clone(), public: *public })
            }
            _ => None,
        })
        .collect();
    wfns.extend(lowered);
    let fns: Vec<(&String, &Vec<(String, WTy)>, &WTy, &String, &String)> = wfns.iter().map(|f| (&f.js, &f.args, &f.ret, &f.text, &f.rs)).collect();
    let public: std::collections::HashSet<&String> = wfns.iter().filter(|f| f.public).map(|f| &f.js).collect();
    if fns.is_empty() {
        return None;
    }
    let mut rs = String::from("// generated by zsc: the wasm layer of a zustscript module (compiled by zustc)\n\n");
    for it in items {
        if let Item::WasmRaw(t) = it {
            rs.push_str(t.trim_matches('\n'));
            rs.push_str("\n\n");
        }
    }
    for (_, _, _, text, _) in &fns {
        rs.push_str(text);
        rs.push_str("\n\n");
    }
    rs.push_str(raw);
    // callbacks: one import per signature, plus the release of an owned one
    let mut cb_sigs: BTreeSet<String> = BTreeSet::new();
    let mut cb_decls = vec![];
    let mut owned = false;
    for (_, args, _, _, _) in &fns {
        for (_, w) in args.iter() {
            if let WTy::Callback(a, r, o) = w {
                owned |= *o;
                let k = sig_key(a, r);
                if cb_sigs.insert(k.clone()) {
                    let ps: Vec<String> = a.iter().enumerate().map(|(i, t)| format!("a{i}: {}", rs_scalar(t))).collect();
                    let ret = if **r == WTy::Unit { String::new() } else { format!(" -> {}", rs_scalar(r)) };
                    cb_decls.push((k, a.clone(), (**r).clone(), format!("fn __zsc_cb_{}(id: u32, {}){ret};", sig_key(a, r), ps.join(", "))));
                }
            }
        }
    }
    {
        // (zustc's runtime has malloc / free inside the module: String and Vec allocate through them, so the
        // glue's buffers and the freeing of returned ones use them too)
        rs.push_str("unsafe extern \"C\" {\n    fn malloc(n: usize) -> *mut u8;\n    fn free(p: *mut u8);\n");
        for d in &cb_decls {
            let _ = writeln!(rs, "    {}", d.3);
        }
        if owned {
            rs.push_str("    fn __zsc_cb_drop(id: u32);\n");
        }
        rs.push_str("}\n\n");
    }
    if owned {
        rs.push_str("/// An owned JS callback: dropping it releases the JS function\nstruct __RsxCb(u32);\nimpl Drop for __RsxCb {\n    fn drop(&mut self) {\n        unsafe { __zsc_cb_drop(self.0) }\n    }\n}\n\n");
    }
    // memory for the boundary
    rs.push_str("#[no_mangle]\npub extern \"C\" fn __zsc_alloc(n: usize) -> *mut u8 {\n    unsafe { malloc(if n == 0 { 1 } else { n }) }\n}\n#[no_mangle]\npub extern \"C\" fn __zsc_free(p: *mut u8) {\n    unsafe { free(p) }\n}\n\n");
    // values that cross as bytes: their schemas (one JS constant each) and the records' Wire impls
    let mut schemas: Vec<String> = vec![];
    let mut wire_recs: BTreeMap<String, &WS> = BTreeMap::new();
    for (_, args, ret, _, _) in &fns {
        for w in args.iter().map(|(_, w)| w).chain(std::iter::once(*ret)) {
            if let WTy::Wire(ws, _, _) = w {
                ws_records(ws, &mut wire_recs);
                let j = ws_js(ws);
                if !schemas.contains(&j) {
                    schemas.push(j);
                }
            }
        }
    }
    let schema_of = |ws: &WS| format!("$zs{}", schemas.iter().position(|j| *j == ws_js(ws)).unwrap());
    if !schemas.is_empty() {
        rs.push_str("mod zustscript_helper {\n");
        // (as a module: no crate docs, no tests)
        for l in HELPER_RS.split("#[cfg(test)]").next().unwrap().lines().filter(|l| !l.starts_with("//!")) {
            rs.push_str(l);
            rs.push('\n');
        }
        rs.push_str("}\n\n");
        for w in wire_recs.values() {
            wire_impl(w, &mut rs);
        }
    }
    let mut glue = String::new();
    let mut names = vec![];
    // (the out area: 3 words for a String / Vec, 8 bytes a slot for a record / enum)
    let mut out_slots = 3;
    for (name, args, ret, _, rs_name) in &fns {
        // (`$` is not in Rust names: the wrapper of `sum$f64` is `__zsc_w_sum__f64`)
        let wname = name.replace('$', "__");
        let mut ps = vec![]; // the wrapper's parameters
        let mut pre = vec![]; // Rust statements before the call
        let mut call = vec![]; // the call's arguments
        let mut jpre = vec![]; // JS before the call
        let mut jargs = vec![]; // the wrapper's arguments in JS
        let mut jpost = vec![]; // JS after the call (frees, releases)
        let out = matches!(ret, WTy::String | WTy::Vec(_) | WTy::TVec(_) | WTy::Wire(..));
        let ret: &WTy = ret;
        if out {
            ps.push("__out: *mut usize".to_string());
            jargs.push("$out".to_string());
        }
        if let WTy::Value(v) = ret {
            out_slots = out_slots.max(v.slots());
            ps.push("__vout: *mut u8".to_string());
            jargs.push("$out".to_string());
        }
        for (a, w) in args.iter() {
            match w {
                WTy::Value(v) => {
                    // a record / enum: one parameter per scalar slot, rebuilt on the Rust side
                    let mut slots = vec![];
                    flat_params(v, a, a, &mut slots);
                    for (rn, rt, js) in slots {
                        ps.push(format!("{rn}: {rt}"));
                        jargs.push(js);
                    }
                    call.push(build(v, a));
                }
                WTy::I32 | WTy::U32 | WTy::F32 | WTy::F64 | WTy::I64 => {
                    ps.push(format!("{a}: {}", rs_scalar(w)));
                    call.push(a.clone());
                    jargs.push(a.clone());
                }
                WTy::Bool => {
                    ps.push(format!("{a}: bool"));
                    call.push(a.clone());
                    jargs.push(format!("{a} ? 1 : 0"));
                }
                WTy::Slice(el) => {
                    let (t, arr, sz) = el.info();
                    ps.push(format!("{a}_p: *const {t}, {a}_n: usize"));
                    call.push(format!("unsafe {{ std::slice::from_raw_parts({a}_p, {a}_n) }}"));
                    jpre.push(format!("const {a}$p = $w.__zsc_alloc(BigInt({a}.length * {sz}));"));
                    jpre.push(format!("new {arr}($w.memory.buffer, Number({a}$p), {a}.length).set({a});"));
                    jargs.push(format!("{a}$p, BigInt({a}.length)"));
                    jpost.push(format!("$w.__zsc_free({a}$p);"));
                }
                WTy::Str => {
                    ps.push(format!("{a}_p: *const u8, {a}_n: usize"));
                    call.push(format!("unsafe {{ std::str::from_utf8_unchecked(std::slice::from_raw_parts({a}_p, {a}_n)) }}"));
                    jpre.push(format!("const {a}$b = $enc.encode({a});"));
                    jpre.push(format!("const {a}$p = $w.__zsc_alloc(BigInt({a}$b.length));"));
                    jpre.push(format!("new Uint8Array($w.memory.buffer, Number({a}$p), {a}$b.length).set({a}$b);"));
                    jargs.push(format!("{a}$p, BigInt({a}$b.length)"));
                    jpost.push(format!("$w.__zsc_free({a}$p);"));
                }
                WTy::Wire(ws, rt, by_ref) => {
                    // as bytes: encoded by zustscript-helper, decoded by zustscript_helper, handed by reference
                    ps.push(format!("{a}_p: *const u8, {a}_n: usize"));
                    // (`__zsc_` names are the generated code's: a parameter cannot take one)
                    pre.push(format!("let __zsc_{a}: {rt} = zustscript_helper::from_bytes(unsafe {{ std::slice::from_raw_parts({a}_p, {a}_n) }});"));
                    call.push(match (&**ws, by_ref) {
                        (WS::Arr(_), _) => format!("&__zsc_{a}[..]"),
                        (_, true) => format!("&__zsc_{a}"),
                        // (a Copy enum, a union of strings: by value)
                        (_, false) => format!("__zsc_{a}"),
                    });
                    jpre.push(format!("const {a}$b = $zsEncode({}, {a});", schema_of(ws)));
                    jpre.push(format!("const {a}$p = $w.__zsc_alloc(BigInt({a}$b.length));"));
                    jpre.push(format!("new Uint8Array($w.memory.buffer, Number({a}$p), {a}$b.length).set({a}$b);"));
                    jargs.push(format!("{a}$p, BigInt({a}$b.length)"));
                    jpost.push(format!("$w.__zsc_free({a}$p);"));
                }
                WTy::Callback(ca, cr, o) => {
                    ps.push(format!("{a}: u32"));
                    let k = sig_key(ca, cr);
                    let cps: Vec<String> = ca.iter().enumerate().map(|(i, t)| format!("a{i}: {}", rs_scalar(t))).collect();
                    let cargs: Vec<String> = (0..ca.len()).map(|i| format!("a{i}")).collect();
                    let cret = if **cr == WTy::Unit { String::new() } else { format!(" -> {}", rs_scalar(cr)) };
                    if *o {
                        pre.push(format!("let {a}_h = __RsxCb({a});"));
                        pre.push(format!(
                            "let {a}_f: Box<dyn Fn({}){cret}> = Box::new(move |{}|{cret} {{ unsafe {{ __zsc_cb_{k}({a}_h.0, {}) }} }});",
                            ca.iter().map(rs_scalar).collect::<Vec<_>>().join(", "),
                            cps.join(", "),
                            cargs.join(", ")
                        ));
                        call.push(format!("{a}_f"));
                        // (released by __zsc_cb_drop when Rust drops it)
                        jpre.push(format!("const {a}$id = $cbAdd({a});"));
                        jargs.push(format!("{a}$id"));
                    } else {
                        pre.push(format!("let {a}_f = |{}|{cret} {{ unsafe {{ __zsc_cb_{k}({a}, {}) }} }};", cps.join(", "), cargs.join(", ")));
                        call.push(format!("&{a}_f"));
                        jpre.push(format!("const {a}$id = $cbAdd({a});"));
                        jargs.push(format!("{a}$id"));
                        // a borrowed callback cannot outlive the call: released here, whatever the wasm side did
                        jpost.push(format!("$cbs.delete({a}$id);"));
                    }
                }
                _ => unreachable!("not an argument type"),
            }
        }
        let wret = match ret {
            WTy::I32 | WTy::U32 | WTy::F32 | WTy::F64 | WTy::I64 | WTy::Bool => format!(" -> {}", rs_scalar(ret)),
            _ => String::new(),
        };
        let _ = writeln!(rs, "#[no_mangle]\npub extern \"C\" fn __zsc_w_{wname}({}){wret} {{", ps.join(", "));
        for p in &pre {
            let _ = writeln!(rs, "    {p}");
        }
        if let WTy::Value(v) = ret {
            let _ = writeln!(rs, "    let r = {rs_name}({});", call.join(", "));
            let mut off = 0;
            store(v, "r", &mut off, 0, &mut rs);
        } else if out {
            let _ = writeln!(rs, "    let r = {rs_name}({});", call.join(", "));
            if let WTy::Wire(..) = ret {
                rs.push_str("    let r = zustscript_helper::to_bytes(&r);\n");
            }
            let ptr = if *ret == WTy::String { "r.as_bytes().as_ptr()" } else { "r.as_ptr()" };
            let _ = write!(rs, "    let (p, l, c) = ({ptr} as usize, r.len(), r.capacity());");
            rs.push_str("\n    std::mem::forget(r);\n    unsafe {\n        *__out = p;\n        *__out.add(1) = l;\n        *__out.add(2) = c;\n    }\n");
        } else {
            let _ = writeln!(rs, "    {rs_name}({})", call.join(", "));
        }
        rs.push_str("}\n\n");

        // the glue
        let jps: Vec<String> = args.iter().map(|(a, _)| a.clone()).collect();
        let _ = writeln!(glue, "{}function {name}({}) {{", if public.contains(name) { "export " } else { "" }, jps.join(", "));
        for l in &jpre {
            let _ = writeln!(glue, "  {l}");
        }
        let callw = format!("$w.__zsc_w_{wname}({})", jargs.join(", "));
        let has_post = !jpost.is_empty();
        if has_post {
            glue.push_str("  try {\n");
        }
        let pad = if has_post { "    " } else { "  " };
        match ret {
            WTy::Value(v) => {
                let _ = writeln!(glue, "{pad}{callw};");
                let _ = writeln!(glue, "{pad}const $o = new DataView($w.memory.buffer, Number($out), {});", 8 * v.slots());
                let mut off = 0;
                let _ = writeln!(glue, "{pad}return {};", decode(v, &mut off));
            }
            WTy::Unit => {
                let _ = writeln!(glue, "{pad}{callw};");
            }
            WTy::U32 => {
                let _ = writeln!(glue, "{pad}return {callw} >>> 0;");
            }
            WTy::Bool => {
                let _ = writeln!(glue, "{pad}return {callw} !== 0;");
            }
            WTy::String | WTy::Vec(_) | WTy::TVec(_) | WTy::Wire(..) => {
                let read = match ret {
                    WTy::Wire(ws, _, _) => format!("$zsDecode({}, new Uint8Array($w.memory.buffer, Number(p), Number(l)))", schema_of(ws)),
                    WTy::TVec(el) => format!("new {}($w.memory.buffer, Number(p), Number(l)).slice()", el.info().1),
                    WTy::Vec(el) => format!("$wToArray(new {}($w.memory.buffer, Number(p), Number(l)))", el.info().1),
                    _ => "$dec.decode(new Uint8Array($w.memory.buffer, Number(p), Number(l)))".to_string(),
                };
                let _ = writeln!(glue, "{pad}{callw};");
                let _ = writeln!(glue, "{pad}const [p, l, c] = new BigUint64Array($w.memory.buffer, Number($out), 3);");
                let _ = writeln!(glue, "{pad}const r = {read};");
                let _ = writeln!(glue, "{pad}if (c !== 0n) $w.__zsc_free(p);");
                let _ = writeln!(glue, "{pad}return r;");
            }
            _ => {
                let _ = writeln!(glue, "{pad}return {callw};");
            }
        }
        if has_post {
            glue.push_str("  } finally {\n");
            for l in &jpost {
                let _ = writeln!(glue, "    {l}");
            }
            glue.push_str("  }\n");
        }
        glue.push_str("}\n\n");
        names.push((*name).clone());
    }
    rs.push_str("fn main() {}\n");

    // the loader and the host side of the imports
    let mut head = String::from("// ---- the wasm layer (compiled by zustc), and its glue\n");
    let _ = writeln!(head, "const $wasmUrl = new URL({:?}, import.meta.url);", format!("./{wasm_file}"));
    head.push_str(
        "const $wasmBytes = typeof process !== \"undefined\" && process.versions?.node\n  ? (await import(\"node:fs\")).readFileSync($wasmUrl)\n  : await (await fetch($wasmUrl)).arrayBuffer();\n",
    );
    head.push_str("const $mod = await WebAssembly.compile($wasmBytes);\n");
    if !schemas.is_empty() {
        // (values that cross as bytes: the format of zustscript-helper, the schemas of this module's)
        head.push_str("import { encode as $zsEncode, decode as $zsDecode } from \"zustscript-helper\";\n");
        for (k, j) in schemas.iter().enumerate() {
            let _ = writeln!(head, "const $zs{k} = {j};");
        }
    }
    head.push_str("const $enc = new TextEncoder();\nconst $dec = new TextDecoder();\n");
    head.push_str("// JS functions handed to wasm, by id (borrowed ones leave when the call returns, owned ones when Rust drops them)\nexport const $cbs = new Map();\nlet $cbNext = 1;\nconst $cbAdd = (f) => {\n  const id = $cbNext++;\n  $cbs.set(id, f);\n  return id;\n};\n");
    head.push_str("const $lines = {};\n");
    head.push_str("function $wToArray(v) {\n  const r = new Array(v.length);\n  for (let i = 0; i < v.length; i++) r[i] = v[i];\n  return r;\n}\n");
    head.push_str("const $env = {\n  __zsc_cb_drop: (id) => void $cbs.delete(id),\n");
    for (k, a, r, _) in &cb_decls {
        let ps: Vec<String> = (0..a.len()).map(|i| format!("a{i}")).collect();
        let conv: Vec<String> = a.iter().enumerate().map(|(i, t)| match t {
            WTy::Bool => format!("a{i} !== 0"),
            WTy::U32 => format!("a{i} >>> 0"),
            _ => format!("a{i}"),
        }).collect();
        let call = format!("$cbs.get(id)({})", conv.join(", "));
        let body = match r {
            WTy::Bool => format!("({call}) ? 1 : 0"),
            WTy::Unit => format!("void {call}"),
            _ => call,
        };
        let _ = writeln!(head, "  __zsc_cb_{k}: (id{}) => {body},", ps.iter().map(|p| format!(", {p}")).collect::<String>());
    }
    head.push_str(
        "  // (what zustc's runtime imports: a panic prints to stderr and aborts)\n  write: (fd, p, n) => {\n    // (line-buffered: a panic message comes in pieces)\n    $lines[fd] = ($lines[fd] || \"\") + $dec.decode(new Uint8Array($w.memory.buffer, Number(p), Number(n)));\n    const k = $lines[fd].lastIndexOf(\"\\n\");\n    if (k >= 0) {\n      (fd === 2 ? console.error : console.log)($lines[fd].slice(0, k));\n      $lines[fd] = $lines[fd].slice(k + 1);\n    }\n    return n;\n  },\n  abort: () => {\n    throw new Error(\"zustscript: the wasm layer panicked\");\n  },\n  exit: (c) => {\n    throw new Error(\"zustscript: the wasm layer exited with \" + c);\n  },\n  fflush: () => 0,\n  sigaction: () => 0,\n};\n",
    );
    head.push_str("for (const i of WebAssembly.Module.imports($mod)) {\n  if (!(i.name in $env)) $env[i.name] = () => {\n    throw new Error(\"zustscript: the wasm layer called \" + i.name + \", which this host does not provide\");\n  };\n}\n");
    head.push_str("const $w = (await WebAssembly.instantiate($mod, { env: $env })).exports;\n");
    let _ = writeln!(head, "const $outSlots = {out_slots};");
    head.push_str("// (the area a wrapper writes a returned String / Vec's (ptr, len, cap) to)\nconst $out = $w.__zsc_alloc(BigInt(8 * $outSlots));\n\n");
    Some(Wasm { rust: rs, glue: format!("{head}{glue}"), names })
}

fn rs_scalar_of(w: &WTy) -> &'static str {
    rs_scalar(w)
}

/// The base name of a Rust type (`ZsOption<f64>` → `ZsOption`): what constructs and matches it, the type
/// arguments inferred
fn base(rt: &str) -> &str {
    rt.split('<').next().unwrap_or(rt)
}

/// The scalar slots of a record / enum argument: (Rust parameter, its type, the JS value)
fn flat_params(v: &VTy, rn: &str, js: &str, out: &mut Vec<(String, String, String)>) {
    match v {
        VTy::Scalar(w) => {
            let j = if *w == WTy::Bool { format!("{js} ? 1 : 0") } else { js.to_string() };
            out.push((rn.to_string(), rs_scalar_of(w).to_string(), j));
        }
        VTy::Record(_, fs) => {
            for (f, fv) in fs {
                flat_params(fv, &format!("{rn}__{f}"), &format!("{js}.{f}"), out);
            }
        }
        VTy::Enum(_, string_enum, vs) => {
            let names: Vec<String> = vs.iter().map(|v| format!("{:?}", v.0)).collect();
            let tag = if *string_enum { format!("[{}].indexOf({js})", names.join(", ")) } else { format!("[{}].indexOf({js}.TAG)", names.join(", ")) };
            out.push((format!("{rn}__tag"), "i32".into(), tag));
            for (vn, fs) in vs {
                for (k, fv) in fs.iter().enumerate() {
                    let mut inner = vec![];
                    flat_params(fv, &format!("{rn}__{vn}_{k}"), &format!("{js}._{k}"), &mut inner);
                    for (r, t, j) in inner {
                        let zero = if t == "i64" { "0n" } else { "0" };
                        out.push((r, t, format!("({js}.TAG === {vn:?} ? {j} : {zero})")));
                    }
                }
            }
        }
    }
}

/// The Rust expression that rebuilds an argument from its slots
fn build(v: &VTy, rn: &str) -> String {
    match v {
        VTy::Scalar(_) => rn.to_string(),
        VTy::Record(rt, fs) => {
            let parts: Vec<String> = fs.iter().map(|(f, fv)| format!("{}: {}", crate::lower::local(f), build(fv, &format!("{rn}__{f}")))).collect();
            format!("{} {{ {} }}", base(rt), parts.join(", "))
        }
        VTy::Enum(rt, _, vs) => {
            let mut s = format!("match {rn}__tag {{ ");
            for (k, (vn, fs)) in vs.iter().enumerate() {
                if fs.is_empty() {
                    let _ = write!(s, "{k} => {}::{vn}, ", base(rt));
                } else {
                    let a: Vec<String> = fs.iter().enumerate().map(|(i, fv)| build(fv, &format!("{rn}__{vn}_{i}"))).collect();
                    let _ = write!(s, "{k} => {}::{vn}({}), ", base(rt), a.join(", "));
                }
            }
            s.push_str("_ => panic!(\"zustscript: not a variant\") }");
            s
        }
    }
}

/// Rust statements that write a result's slots to `__vout` (8 bytes a slot: f64 as f64, the others as i64)
fn store(v: &VTy, e: &str, off: &mut usize, depth: usize, out: &mut String) {
    match v {
        VTy::Scalar(w) => {
            let t = if matches!(w, WTy::F64 | WTy::F32) { "f64" } else { "i64" };
            let _ = writeln!(out, "    unsafe {{ *(__vout as *mut {t}).add({off}) = ({e}) as {t}; }}");
            *off += 1;
        }
        VTy::Record(_, fs) => {
            for (f, fv) in fs {
                store(fv, &format!("{e}.{}", crate::lower::local(f)), off, depth, out);
            }
        }
        VTy::Enum(rt, _, vs) => {
            let tag = *off;
            *off += 1;
            let _ = writeln!(out, "    match {e} {{");
            for (k, (vn, fs)) in vs.iter().enumerate() {
                let binds: Vec<String> = (0..fs.len()).map(|i| format!("__v{depth}_{i}")).collect();
                if fs.is_empty() {
                    let _ = writeln!(out, "    {}::{vn} => {{", base(rt));
                } else {
                    let _ = writeln!(out, "    {}::{vn}({}) => {{", base(rt), binds.join(", "));
                }
                let _ = writeln!(out, "    unsafe {{ *(__vout as *mut i64).add({tag}) = {k}; }}");
                for (fv, b) in fs.iter().zip(&binds) {
                    store(fv, b, off, depth + 1, out);
                }
                out.push_str("    }\n");
            }
            out.push_str("    }\n");
        }
    }
}

/// The JS expression that reads a result back from the out area (`$o`, a DataView of it)
fn decode(v: &VTy, off: &mut usize) -> String {
    match v {
        VTy::Scalar(w) => {
            let at = 8 * *off;
            *off += 1;
            match w {
                WTy::F64 | WTy::F32 => format!("$o.getFloat64({at}, true)"),
                WTy::I64 => format!("$o.getBigInt64({at}, true)"),
                WTy::Bool => format!("$o.getBigInt64({at}, true) !== 0n"),
                _ => format!("Number($o.getBigInt64({at}, true))"),
            }
        }
        VTy::Record(_, fs) => {
            let parts: Vec<String> = fs.iter().map(|(f, fv)| format!("{f}: {}", decode(fv, off))).collect();
            format!("{{ {} }}", parts.join(", "))
        }
        VTy::Enum(_, string_enum, vs) => {
            let tag = format!("Number($o.getBigInt64({}, true))", 8 * *off);
            *off += 1;
            if *string_enum {
                let names: Vec<String> = vs.iter().map(|v| format!("{:?}", v.0)).collect();
                return format!("[{}][{tag}]", names.join(", "));
            }
            let mut alts = vec![];
            for (vn, fs) in vs {
                let mut o = format!("{{ TAG: {vn:?}");
                for (k, fv) in fs.iter().enumerate() {
                    let _ = write!(o, ", _{k}: {}", decode(fv, off));
                }
                o.push_str(" }");
                alts.push(o);
            }
            let mut s = alts.pop().unwrap();
            for (k, a) in alts.into_iter().enumerate().rev() {
                s = format!("{tag} === {k} ? {a} : {s}");
            }
            format!("({s})")
        }
    }
}
