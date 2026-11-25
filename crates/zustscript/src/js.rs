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
    /// `return` here returns from a generator handler's arm itself: `return k(x)` resumes as the arm's last act
    arm_tail: bool,
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
    /// import the helpers from the shared `zsc-runtime.js` (the default), or write them into the module
    pub shared_runtime: bool,
    pub wasm_wanted: Vec<(String, Vec<Ty>)>,
    /// the `push` / `concat` calls of the function being written that append in place ([`crate::own::inplace_pushes`])
    inplace: HashSet<Id>,
    /// the comparators written for `==` by content (one per type, `$eq1` ..), and their definitions
    eqs: HashMap<Ty, String>,
    eq_defs: String,
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

/// The text of a template literal: `` ` ``, `\\` and `${` escaped, and the characters a string literal escapes
fn template_text(s: &str) -> String {
    let mut o = String::new();
    let cs: Vec<char> = s.chars().collect();
    for (i, &c) in cs.iter().enumerate() {
        match c {
            '`' => o.push_str("\\`"),
            '\\' => o.push_str("\\\\"),
            '$' if cs.get(i + 1) == Some(&'{') => o.push_str("\\$"),
            // (a line break is escaped too: the output is re-indented in places (`Js::iife`), which would add to
            // a literal one)
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            // (`\0` before a digit would be an octal escape, an error in a template)
            '\0' => o.push_str("\\x00"),
            '\u{2028}' => o.push_str("\\u2028"),
            '\u{2029}' => o.push_str("\\u2029"),
            c => o.push(c),
        }
    }
    o
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
    // (appends `ys` onto `xs` in place: only onto an array no one else can see, see own::inplace_pushes)
    ("$append", "function $append(xs, ys) {\n  for (let i = 0; i < ys.length; i++) xs.push(ys[i]);\n  return xs;\n}"),
    ("$fold", "function $fold(xs, acc, f) {\n  for (let i = 0; i < xs.length; i++) acc = f(acc, xs[i]);\n  return acc;\n}"),
    ("$rangeFold", "function $rangeFold(a, b, acc, f) {\n  for (let i = a; i < b; i++) acc = f(acc, i);\n  return acc;\n}"),
    ("$rangeMap", "function $rangeMap(a, b, f) {\n  const r = new Array(Math.max(b - a, 0));\n  for (let i = a; i < b; i++) r[i - a] = f(i);\n  return r;\n}"),
    ("$rangeMapTo", "function $rangeMapTo(T, a, b, f) {\n  const r = new T(Math.max(b - a, 0));\n  for (let i = a; i < b; i++) r[i - a] = f(i);\n  return r;\n}"),
    ("$arity1", "function $arity1(f) {\n  return (a) => f(a);\n}"),
    ("$arity2", "function $arity2(f) {\n  return (a, b) => f(a, b);\n}"),
    ("$round", "function $round(x) {\n  return x < 0 ? -Math.round(-x) : Math.round(x);\n}"),
    // (the methods of the prelude's `Option` / `Result`: `None` / `Err` passes through as it is)
    ("$o_map", "function $o_map(o, f) {\n  return o.TAG === \"None\" || o.TAG === \"Err\" ? o : { TAG: o.TAG, _0: f(o._0) };\n}"),
    ("$o_then", "function $o_then(o, f) {\n  return o.TAG === \"None\" || o.TAG === \"Err\" ? o : f(o._0);\n}"),
    ("$o_filter", "function $o_filter(o, f) {\n  return o.TAG === \"Some\" && !f(o._0) ? { TAG: \"None\" } : o;\n}"),
    ("$o_map_err", "function $o_map_err(r, f) {\n  return r.TAG === \"Err\" ? { TAG: \"Err\", _0: f(r._0) } : r;\n}"),
    ("$o_unwrap_or", "function $o_unwrap_or(o, d) {\n  return o.TAG === \"None\" || o.TAG === \"Err\" ? d : o._0;\n}"),
    ("$o_unwrap_or_else", "function $o_unwrap_or_else(o, f) {\n  return o.TAG === \"None\" ? f() : o.TAG === \"Err\" ? f(o._0) : o._0;\n}"),
    ("$o_ok_or", "function $o_ok_or(o, e) {\n  return o.TAG === \"Some\" ? { TAG: \"Ok\", _0: o._0 } : { TAG: \"Err\", _0: e };\n}"),
    ("$o_ok_or_else", "function $o_ok_or_else(o, f) {\n  return o.TAG === \"Some\" ? { TAG: \"Ok\", _0: o._0 } : { TAG: \"Err\", _0: f() };\n}"),
    ("$o_ok", "function $o_ok(r) {\n  return r.TAG === \"Ok\" ? { TAG: \"Some\", _0: r._0 } : { TAG: \"None\" };\n}"),
    // (what JS gives an extern: `null` / `undefined` is `None`; a `Result` catches what it throws, as its message or
    // as the value thrown; a `Promise` of one, its rejection too. And an `Option` handed to JS: `undefined` for `None`)
    ("$nullable", "function $nullable(x) {\n  return x == null ? { TAG: \"None\" } : { TAG: \"Some\", _0: x };\n}"),
    ("$undef", "function $undef(o) {\n  return o.TAG === \"Some\" ? o._0 : undefined;\n}"),
    // (the message of a value that cannot be made a string, `Object.create(null)`, is fixed: still an `Err`)
    ("$message", "function $message(e) {\n  try {\n    return e instanceof Error ? String(e.message) : String(e);\n  } catch {\n    return \"(a thrown value that has no string form)\";\n  }\n}"),
    ("$catch", "function $catch(f, message) {\n  try {\n    return { TAG: \"Ok\", _0: f() };\n  } catch (e) {\n    return { TAG: \"Err\", _0: message ? $message(e) : e };\n  }\n}"),
    ("$catchP", "async function $catchP(f, message) {\n  try {\n    return { TAG: \"Ok\", _0: await f() };\n  } catch (e) {\n    return { TAG: \"Err\", _0: message ? $message(e) : e };\n  }\n}"),
    // (`xs.try_map(f)`: one array, built once; the first `Err` / `None` is the result)
    ("$try_map_r", "function $try_map_r(xs, f) {\n  const out = [];\n  for (let i = 0; i < xs.length; i++) {\n    const r = f(xs[i]);\n    if (r.TAG === \"Err\") return r;\n    out.push(r._0);\n  }\n  return { TAG: \"Ok\", _0: out };\n}"),
    ("$try_map_o", "function $try_map_o(xs, f) {\n  const out = [];\n  for (let i = 0; i < xs.length; i++) {\n    const r = f(xs[i]);\n    if (r.TAG === \"None\") return r;\n    out.push(r._0);\n  }\n  return { TAG: \"Some\", _0: out };\n}"),
    (
        "$handle",
        "// Deep handlers on generators (one-shot continuations), run by one loop that keeps the frames on a heap stack:\n// operations and resumptions do not grow the JS stack. A generator yields an operation (a variant) or an\n// instruction to the loop (`$i`): run a handled body, resume a continuation, or resume it as the arm's last act\nfunction $handle(body, ops, arm) {\n  return { $i: 1, body, ops, arm };\n}\n\nfunction $resume(k, v) {\n  return { $i: 2, k, v };\n}\n\nfunction $tail(k, v) {\n  return { $i: 3, k, v };\n}\n\n// The loop over the frames `st` (a frame: a generator, and for a handled body its operations and arm; a\n// continuation: the frames it cut off). An operation no frame handles is yielded to whoever drives the loop (a TS\n// caller of an exported function, or an outer loop), and its answer is sent back to the frame that performed it\nfunction* $drive(st) {\n  let v, e, thrown = false;\n  for (;;) {\n    const f = st[st.length - 1];\n    let r;\n    try {\n      r = thrown ? f.g.throw(e) : f.g.next(v);\n    } catch (x) {\n      st.pop();\n      if (st.length === 0) throw x;\n      thrown = true;\n      e = x;\n      continue;\n    }\n    thrown = false;\n    v = undefined;\n    if (r.done) {\n      st.pop();\n      if (st.length === 0) return r.value;\n      v = r.value;\n      continue;\n    }\n    const y = r.value;\n    if (y.$i === 1) {\n      st.push({ g: y.body(), ops: y.ops, arm: y.arm });\n    } else if (y.$i !== undefined) {\n      const seg = y.k.seg;\n      if (seg === null) {\n        thrown = true;\n        e = new Error(\"zustscript: a continuation was resumed twice\");\n        continue;\n      }\n      y.k.seg = null;\n      // (as the arm's last act, the arm's value is the continuation's: its frame goes)\n      if (y.$i === 3) st.pop();\n      for (let i = 0; i < seg.length; i++) st.push(seg[i]);\n      v = y.v;\n    } else {\n      // (the nearest handler of the operation: the frames from it up are the continuation, and its arm runs in\n      // their place, under the handlers outside it)\n      let i = st.length - 1;\n      while (i >= 0 && (st[i].ops === undefined || !st[i].ops.includes(y.TAG))) i--;\n      if (i < 0) {\n        try {\n          v = yield y;\n        } catch (x) {\n          thrown = true;\n          e = x;\n        }\n        continue;\n      }\n      const h = st[i];\n      const k = { seg: st.splice(i) };\n      st.push({ g: h.arm(y, k) });\n    }\n  }\n}\n\n// A `handle` outside any generator: nothing outside it can answer an operation it does not handle\nfunction $run(body, ops, arm) {\n  const r = $drive([{ g: body(), ops, arm }]).next();\n  if (!r.done) throw new Error(\"zustscript: unhandled effect \" + r.value.TAG);\n  return r.value;\n}",
    ),
    (
        "$perform",
        "// Direct (tail-resumptive) handlers: a chain of frames; an operation calls the nearest frame's arm, which runs\n// under the handlers outside its own `handle` (deep handler semantics)\nlet $cur = null;\nfunction $perform(op, ...a) {\n  for (let f = $cur; f !== null; f = f.p) {\n    const h = f.h[op];\n    if (h !== undefined) {\n      const s = $cur;\n      $cur = f.p;\n      try {\n        return h(...a);\n      } finally {\n        $cur = s;\n      }\n    }\n  }\n  throw new Error(\"zustscript: unhandled effect \" + op);\n}\n\nfunction $handleD(h, body) {\n  const f = { h, p: $cur };\n  $cur = f;\n  try {\n    return body();\n  } finally {\n    $cur = f.p;\n  }\n}",
    ),
];

impl<'a> Js<'a> {
    pub fn new(info: &'a Info, jsx_runtime: &str) -> Js<'a> {
        Js {
            info,
            out: String::new(),
            ind: 0,
            tmp: 0,
            gen_: false,
            arm_tail: false,
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
            shared_runtime: true,
            wasm_wanted: vec![],
            inplace: HashSet::new(),
            eqs: HashMap::new(),
            eq_defs: String::new(),
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
        e.unit_only() && !e.is_effect() && e.ts.as_ref().is_none_or(|t| t.key.is_none())
    }

    /// The JS value of variant `v`'s tag: its own name, or the TS union's string for it (`"circle"`)
    fn tag_js(&self, en: &str, v: &str) -> String {
        let e = &self.info.enums[en];
        match &e.ts {
            Some(t) => js_str(&t.tags[e.variants.iter().position(|x| x.0 == v).unwrap()]),
            None => js_str(v),
        }
    }

    /// The property that holds the tag: `TAG`, or the TS union's tag field (`kind`)
    fn tag_key(&self, en: &str) -> String {
        match self.info.enums[en].ts.as_ref().and_then(|t| t.key.clone()) {
            Some(k) => k,
            None => "TAG".into(),
        }
    }

    /// The property that holds field `k` of variant `v`: `_0`, or the TS member's name (`r`)
    fn field_key(&self, en: &str, v: &str, k: usize) -> String {
        let e = &self.info.enums[en];
        match &e.ts {
            Some(t) => t.fields[e.variants.iter().position(|x| x.0 == v).unwrap()][k].clone(),
            None => format!("_{k}"),
        }
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
                    self.module_names.insert(js_root(js).to_string());
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
        let eqs = std::mem::take(&mut self.eq_defs);
        self.out.push_str(&eqs);
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
        let params: Vec<String> = args.iter().map(|a| a.0.clone()).collect();
        self.inplace = crate::own::inplace_pushes(&params, body, self.info);
        let saved = std::mem::take(&mut self.out);
        let is_async = matches!(it, Item::Fn { is_async: true, .. });
        self.line(&format!("{}{}function{} {js_name}({}) {{", if public { "export " } else { "" }, if is_async { "async " } else { "" }, if gen_ { "*" } else { "" }, ps.join(", ")));
        self.ind += 1;
        self.gen_ = gen_;
        self.tmp = 0;
        // (a TS caller drives an exported generator: its body runs under the handlers' loop, so the caller sees only the
        // operations the function leaves unhandled, never the loop's instructions)
        let drive = gen_ && public;
        if drive {
            self.helpers.insert("$handle");
            self.line("return yield* $drive([{ g: (function* () {");
            self.ind += 1;
        }
        self.stmt_expr(body, &Dest::Return);
        if drive {
            self.ind -= 1;
            self.line("})() }]);");
        }
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
            need.extend(["$resume", "$tail", "$drive", "$run"]);
        }
        if need.contains("$catch") || need.contains("$catchP") {
            need.insert("$message");
        }
        if need.contains("$perform") {
            need.insert("$handleD");
        }
        if self.shared_runtime {
            if !need.is_empty() {
                let _ = writeln!(h, "import {{ {} }} from \"./{RUNTIME}.js\";", need.iter().copied().collect::<Vec<_>>().join(", "));
            }
            return h;
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
        // (resuming as the arm's last act: the loop drops the arm's frame, so tail-resumptive arms do not pile up)
        let v = if matches!(d, Dest::Return) && self.arm_tail && v.starts_with("(yield $resume(") && v.ends_with(')') && balanced(&v[1..v.len() - 1]) {
            format!("(yield $tail({}", &v["(yield $resume(".len()..])
        } else {
            v
        };
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
            // (a `?` in a branch runs only on that branch: not a `? :` whose operands were written before it)
            EK::If(_, t, el) => Self::complex(t) || el.as_ref().is_none_or(|x| Self::complex(x)) || has_try(t) || el.as_deref().is_some_and(has_try),
            _ => false,
        }
    }

    /// `s` kept in a temporary first, when a later operand returns early with `?`: the `?` is written as statements
    /// before the expression, so an operand before it is evaluated before them too (left to right, as written)
    fn pin(&mut self, s: String, later: bool) -> String {
        if !later || s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            return s;
        }
        let t = self.fresh();
        self.line(&format!("const {t} = {s};"));
        t
    }

    /// The operands `xs`, in order (each kept in a temporary before a later one's `?`)
    fn seq(&mut self, xs: &[&Expr]) -> Vec<String> {
        let last = xs.iter().rposition(|y| has_try(y));
        let mut out = vec![];
        for (k, x) in xs.iter().enumerate() {
            let s = self.expr(x);
            let later: &[&Expr] = match last {
                Some(l) if k < l => &xs[k + 1..=l],
                _ => &[],
            };
            out.push(self.pin_before(x, s, later));
        }
        out
    }

    /// The JS `s` of the operand `x`, kept in a temporary when one of the operands after it (`later`) has a `?`. A
    /// literal needs none; a local's name needs one only when a later operand assigns a local
    fn pin_before(&mut self, x: &Expr, s: String, later: &[&Expr]) -> String {
        if !later.iter().any(|y| has_try(y)) || matches!(x.kind, EK::Lit(_)) {
            return s;
        }
        if matches!(x.kind, EK::Var(_)) && later.iter().any(|y| assigns_local(y)) {
            let t = self.fresh();
            self.line(&format!("const {t} = {s};"));
            return t;
        }
        self.pin(s, true)
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
                    // (a condition with a `?` is written before its `if`: inside the `else`)
                    if let EK::If(c2, t2, el2) = &x.kind
                        && !has_try(c2)
                    {
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
                    return vec![format!("{s} === {}", self.tag_js(en, v))];
                }
                let mut c = vec![format!("{s}.{} === {}", self.tag_key(en), self.tag_js(en, v))];
                for (k, sp) in subs.iter().enumerate() {
                    c.extend(self.pat_cond(sp, &format!("{s}.{}", self.field_key(en, v, k))));
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
            Pat::Variant(_, v, subs, _) => {
                let en = self.info.variant_of[v].clone();
                for (k, p) in subs.iter().enumerate() {
                    self.pat_binds(p, &format!("{s}.{}", self.field_key(&en, v, k)), out);
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
            EK::Var(_) | EK::Path(..) | EK::Call(..) | EK::Method(..) | EK::Field(..) | EK::Array(_) | EK::Tuple(_) | EK::Record(..) | EK::Jsx(_) | EK::Interp(..) => 17,
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
        let saved = (std::mem::take(&mut self.out), self.ind, std::mem::take(&mut self.conts), std::mem::take(&mut self.dconts), self.gen_, std::mem::replace(&mut self.arm_tail, false));
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
            // (a computed key: an operation named `__proto__` is an own property, not the table's prototype)
            parts.push(format!("[{}]: ({}) => {{\n{b}{pad}}}", js_str(&a.variant), ps.join(", ")));
        }
        self.ind = saved.1 + 1;
        self.dconts.clear();
        self.stmt_expr(body, &Dest::Return);
        let b = std::mem::take(&mut self.out);
        (self.out, self.ind, self.conts, self.dconts, self.gen_, self.arm_tail) = saved;
        // (a null prototype: an operation named like an Object method, `toString`, must not find an inherited one)
        format!("$handleD({{ __proto__: null, {} }}, () => {{\n{b}{pad}}})", parts.join(", "))
    }

    fn iife(&mut self, e: &Expr) -> String {
        // (a `?` inside returns from the function being written: the expression is written as statements before
        // the one it is in, into a temporary, not as a function of its own)
        if has_try(e) {
            let v = self.fresh();
            self.line(&format!("let {v};"));
            self.stmt_expr(e, &Dest::Assign(v.clone()));
            return v;
        }
        if let Some(pos) = loop_exit(e) {
            self.errors.push((pos, "`break` / `continue` here would have to leave a function in the JS output: move this `if` / `match` into a statement of its own (`let v = if ..;`)".into()));
        }
        let save = (std::mem::take(&mut self.out), self.ind, std::mem::replace(&mut self.arm_tail, false));
        self.ind = 1;
        let boxed = !self.gen_ && awaits(e);
        if boxed {
            // (assigned, then returned in a box: see below)
            let v = self.fresh();
            self.line(&format!("let {v};"));
            self.stmt_expr(e, &Dest::Assign(v.clone()));
            self.line(&format!("return [{v}];"));
        } else {
            self.stmt_expr(e, &Dest::Return);
        }
        let body = std::mem::replace(&mut self.out, save.0);
        self.ind = save.1;
        self.arm_tail = save.2;
        let pad = "  ".repeat(self.ind);
        let body: String = body.lines().map(|l| format!("{pad}{l}\n")).collect();
        if self.gen_ {
            format!("(yield* (function* () {{\n{body}{pad}}})())")
        } else if boxed {
            // (an `await` inside: the function is async, and its Promise is awaited where it stands. The value is
            // boxed: a Promise the expression gives would be adopted by the async function and come out awaited)
            format!("(await (async () => {{\n{body}{pad}}})())[0]")
        } else {
            format!("(() => {{\n{body}{pad}}})()")
        }
    }

    fn num(&self, id: Id) -> Ty {
        subst_ty(&self.info.num.get(&id).cloned().unwrap_or(Ty::F64), &self.subst)
    }

    fn variant_value(&mut self, en: &str, v: &str, args: Vec<String>) -> String {
        if self.string_enum(en) {
            return self.tag_js(en, v);
        }
        // (a union read from TS: TS's own shape, `{ kind: "circle", r: 1 }`)
        let mut s = format!("{{ {}: {}", self.tag_key(en), self.tag_js(en, v));
        for (k, a) in args.iter().enumerate() {
            let _ = write!(s, ", {}: {a}", self.field_key(en, v, k));
        }
        s.push_str(" }");
        s
    }

    /// A function value passed to a builtin that calls it with extra arguments (`map` passes the index): a closure
    /// literal as is, anything else wrapped so it receives exactly its own arguments
    fn callback(&mut self, f: &Expr, arity: usize) -> String {
        let s = self.expr(f);
        self.adapt(f, s, arity)
    }

    /// The JS `s` of the function value `f`, made to take exactly `arity` arguments (see [`Js::callback`])
    fn adapt(&mut self, f: &Expr, s: String, arity: usize) -> String {
        // (a closure, or an extern's converting function, takes the arguments it names and no more)
        if matches!(f.kind, EK::Closure(..)) || matches!(self.info.callee.get(&f.id), Some(Callee::Extern(_))) && s.starts_with("($a") {
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
                    // (as a value: a function that converts as a call does)
                    let (sig, _, _) = &self.info.externs[n];
                    let (args, ret) = (sig.args.clone(), sig.ret.clone());
                    // (a constructor is called with `new`: a function of its own)
                    let ctor = js.starts_with("new ");
                    let zsc = self.info.zsc_externs.contains(n);
                    if !ctor && (zsc || (!args.iter().any(is_option) && !needs_js_result(&ret))) {
                        return js;
                    }
                    let ps: Vec<String> = (0..args.len()).map(|k| format!("$a{k}")).collect();
                    let a: Vec<String> = if zsc { ps.clone() } else { ps.iter().zip(&args).map(|(p, t)| self.js_arg(p.clone(), t)).collect() };
                    let call = format!("{js}({})", a.join(", "));
                    let body = if zsc { call } else { self.js_result(call, &ret) };
                    format!("({}) => {body}", ps.join(", "))
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
                // (a computed callee is evaluated before the arguments, as JS does)
                let callee = if matches!(f.kind, EK::Var(_) | EK::Path(..)) {
                    None
                } else {
                    let c = self.expr(f);
                    Some(self.pin_before(f, c, &args.iter().collect::<Vec<_>>()))
                };
                let a: Vec<String> = self.seq(&args.iter().collect::<Vec<_>>());
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
                            let x = a.first().cloned().unwrap_or_else(|| "undefined".into());
                            return format!("(yield $resume({}, {x}))", self.local(n));
                        }
                        Some(Callee::Extern(js)) => {
                            self.note_extern(n);
                            if self.info.zsc_externs.contains(n) {
                                return format!("{js}({})", a.join(", "));
                            }
                            let (sig, _, _) = &self.info.externs[n];
                            let (args, ret) = (sig.args.clone(), sig.ret.clone());
                            let a: Vec<String> = a.into_iter().zip(&args).map(|(v, t)| self.js_arg(v, t)).collect();
                            return self.extern_call(a, &ret, |a| format!("{js}({})", a.join(", ")));
                        }
                        _ => {}
                    }
                }
                let f = callee.unwrap_or_else(|| self.expr(f));
                format!("{}({})", self.wrap_callee(&f), a.join(", "))
            }
            EK::Method(r, m, args) => {
                let b = self.info.builtin.get(&e.id).copied();
                // `range(a, b).fold(..)` / `.map(..)`: a counting loop, no array of indices
                if let EK::Call(f, ra) = &r.kind
                    && matches!(self.info.callee.get(&f.id), Some(Callee::Range))
                    && matches!(b, Some(Builtin::Fold | Builtin::Map))
                {
                    // (the bounds, a fold's initial value, then the function, in order)
                    let fold = b == Some(Builtin::Fold);
                    let mut ops = vec![&ra[0], &ra[1]];
                    ops.extend(args.iter());
                    let v = self.seq(&ops);
                    let (lo, hi) = (v[0].clone(), v[1].clone());
                    let fe = &args[if fold { 1 } else { 0 }];
                    let f = self.adapt(fe, v[v.len() - 1].clone(), if fold { 2 } else { 1 });
                    return if fold {
                        self.helpers.insert("$rangeFold");
                        let init = v[2].clone();
                        format!("$rangeFold({lo}, {hi}, {init}, {f})")
                    } else if let Some(el) = self.info.typed_lit.get(&e.id).cloned() {
                        // (into a typed array, as the type says)
                        self.helpers.insert("$rangeMapTo");
                        format!("$rangeMapTo({}, {lo}, {hi}, {f})", typed_js(&el))
                    } else {
                        self.helpers.insert("$rangeMap");
                        format!("$rangeMap({lo}, {hi}, {f})")
                    };
                }
                let recv = self.operand(r);
                let recv = self.pin_before(r, recv, &args.iter().collect::<Vec<_>>());
                // (a method of an extern type: called as JS's, its arguments and result converted)
                if let Some((tn, mn)) = self.info.member.get(&e.id).cloned() {
                    let (_, ps, ret, js) = self.info.ext_types[&tn].methods.iter().find(|m| m.0 == mn).cloned().unwrap();
                    let a: Vec<String> = self.seq(&args.iter().collect::<Vec<_>>());
                    let a: Vec<String> = a.into_iter().zip(&ps).map(|(v, t)| self.js_arg(v, t)).collect();
                    if let Some(p) = js.strip_suffix('=') {
                        return format!("({}{} = {}, undefined)", recv, member(p), a[0]);
                    }
                    let vals: Vec<String> = [recv].into_iter().chain(a).collect();
                    return self.extern_call(vals, &ret, |v| format!("{}{}({})", v[0], member(&js), v[1..].join(", ")));
                }
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
                        // (the initial value, then the function, in order)
                        let v = self.seq(&[&args[0], &args[1]]);
                        let f = self.adapt(&args[1], v[1].clone(), 2);
                        format!("$fold({recv}, {}, {f})", v[0])
                    }
                    // (in place onto a local array no one else sees, at its last use: amortized O(1), not a copy)
                    Some(B::Push) if self.inplace.contains(&e.id) => format!("({recv}.push({}), {recv})", arg(self, 0)),
                    Some(B::Concat) if self.inplace.contains(&e.id) => {
                        self.helpers.insert("$append");
                        format!("$append({recv}, {})", arg(self, 0))
                    }
                    Some(B::Push) => format!("[...{recv}, {}]", arg(self, 0)),
                    Some(B::Concat) => format!("[...{recv}, ...{}]", self.operand(&args[0])),
                    Some(B::Join) => format!("{recv}.join({})", arg(self, 0)),
                    Some(B::ToUpper) => format!("{recv}.toUpperCase()"),
                    Some(B::Trim) => format!("{recv}.trim()"),
                    Some(B::Contains) => format!("{recv}.includes({})", arg(self, 0)),
                    Some(B::Split) => format!("{recv}.split({})", arg(self, 0)),
                    Some(B::ToString) => format!("String({})", unparen(&recv)),
                    Some(B::Floor) => format!("Math.floor({})", unparen(&recv)),
                    Some(B::Sqrt) if self.num(e.id) == Ty::F32 => format!("Math.fround(Math.sqrt({}))", unparen(&recv)),
                    Some(B::Sqrt) => format!("Math.sqrt({})", unparen(&recv)),
                    Some(B::Max) => format!("Math.max({}, {})", unparen(&recv), arg(self, 0)),
                    Some(B::Min) => format!("Math.min({}, {})", unparen(&recv), arg(self, 0)),
                    Some(B::ToArray) => format!("Array.from({recv})"),
                    Some(B::Abs) => format!("Math.abs({})", unparen(&recv)),
                    Some(B::Round) => {
                        self.helpers.insert("$round");
                        format!("$round({})", unparen(&recv))
                    }
                    Some(B::Opt(m @ ("is_some" | "is_none" | "is_ok" | "is_err"))) => {
                        let tag = match m {
                            "is_some" => "Some",
                            "is_none" => "None",
                            "is_ok" => "Ok",
                            _ => "Err",
                        };
                        format!("({recv}.TAG === {})", js_str(tag))
                    }
                    Some(B::Opt(m)) => {
                        let h = match m {
                            "map" => "$o_map",
                            "and_then" => "$o_then",
                            "filter" => "$o_filter",
                            "map_err" => "$o_map_err",
                            "unwrap_or" => "$o_unwrap_or",
                            "unwrap_or_else" => "$o_unwrap_or_else",
                            "ok_or" => "$o_ok_or",
                            "ok_or_else" => "$o_ok_or_else",
                            _ => "$o_ok",
                        };
                        self.helpers.insert(h);
                        let a: Vec<String> = match m {
                            "map" | "and_then" | "filter" | "map_err" => vec![self.callback(&args[0], 1)],
                            _ => args.iter().map(|x| self.expr(x)).collect(),
                        };
                        format!("{h}({})", [recv].into_iter().chain(a).collect::<Vec<_>>().join(", "))
                    }
                    Some(B::TryMap(res)) => {
                        let h = if res { "$try_map_r" } else { "$try_map_o" };
                        self.helpers.insert(h);
                        format!("{h}({recv}, {})", self.callback(&args[0], 1))
                    }
                    None => {
                        let a: Vec<String> = self.seq(&args.iter().collect::<Vec<_>>());
                        format!("{recv}.{m}({})", a.join(", "))
                    }
                }
            }
            EK::Index(x, i) => {
                self.helpers.insert("$at");
                let v = self.seq(&[x, i]);
                format!("$at({}, {})", v[0], v[1])
            }
            EK::Field(x, f) if self.info.member.contains_key(&e.id) => {
                let (tn, pn) = self.info.member[&e.id].clone();
                let (_, t, js) = self.info.ext_types[&tn].props.iter().find(|p| p.0 == pn).cloned().unwrap();
                let s = self.operand(x);
                // (a getter that throws: the receiver is evaluated outside the catch, as a call's are)
                self.extern_call(vec![s], &t, |v| format!("{}{}", v[0], member(&js)))
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
                let saved = (self.gen_, std::mem::take(&mut self.conts), std::mem::take(&mut self.dconts), std::mem::replace(&mut self.arm_tail, false));
                self.gen_ = false;
                let mark = self.locals.len();
                let ps: Vec<String> = ps.iter().map(|p| self.bind(&p.0)).collect();
                let b = if Self::complex(body) || has_try(body) {
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
                (self.gen_, self.conts, self.dconts, self.arm_tail) = saved;
                self.locals.truncate(mark);
                format!("({}) => {b}", ps.join(", "))
            }
            EK::Record(_, fs, base) => {
                let mut parts = vec![];
                if let Some(b) = base {
                    let v = self.operand(b);
                    let v = self.pin_before(b, v, &fs.iter().map(|f| &f.1).collect::<Vec<_>>());
                    parts.push(format!("...{v}"));
                }
                let vs = self.seq(&fs.iter().map(|f| &f.1).collect::<Vec<_>>());
                for ((f, _), v) in fs.iter().zip(vs) {
                    parts.push(if v == *f { f.clone() } else { format!("{f}: {v}") });
                }
                format!("{{ {} }}", parts.join(", "))
            }
            EK::Array(xs) if self.info.typed_lit.contains_key(&e.id) => {
                let ctor = typed_js(&self.info.typed_lit[&e.id].clone());
                format!("new {ctor}([{}])", self.seq(&xs.iter().collect::<Vec<_>>()).join(", "))
            }
            EK::Array(xs) | EK::Tuple(xs) => format!("[{}]", self.seq(&xs.iter().collect::<Vec<_>>()).join(", ")),
            EK::Await(x) => format!("(await {})", self.bin_operand(x, 14, true)),
            // (statements before the one it is in: the value in a temporary, returned as it is when it is the
            // `Err` / `None`, whose shape does not depend on the other type argument)
            EK::Try(x) => {
                let v = self.expr(x);
                let t = self.fresh();
                self.line(&format!("const {t} = {v};"));
                let en = self.info.tries[&e.id].clone();
                let bad = if en == "Result" { "Err" } else { "None" };
                let tag = self.tag_key(&en);
                self.line(&format!("if ({t}.{tag} === {}) return {t};", self.tag_js(&en, bad)));
                format!("{t}.{}", self.field_key(&en, if en == "Result" { "Ok" } else { "Some" }, 0))
            }
            EK::Async(c) => format!("async {}", self.expr(c)),
            // (a template literal: a hole writes its value as `String(..)` does, which is `to_string`'s JS)
            EK::Interp(parts, holes) => {
                let hs = self.seq(&holes.iter().collect::<Vec<_>>());
                let mut o = String::from("`");
                for (k, p) in parts.iter().enumerate() {
                    o.push_str(&template_text(p));
                    if let Some(h) = hs.get(k) {
                        let _ = write!(o, "${{{h}}}");
                    }
                }
                o.push('`');
                o
            }
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
                let a: Vec<String> = self.seq(&args.iter().collect::<Vec<_>>());
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
                let saved = (self.gen_, self.conts.clone(), std::mem::take(&mut self.out), self.ind, self.arm_tail);
                self.gen_ = true;
                self.arm_tail = false;
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
                    self.arm_tail = true;
                    self.stmt_expr(&a.body, &Dest::Return);
                    self.arm_tail = false;
                    self.conts.pop();
                    self.locals.truncate(mark);
                    self.ind -= 1;
                }
                self.line("}");
                let arms_js = std::mem::take(&mut self.out);
                let pad = "  ".repeat(saved.3);
                (self.gen_, self.conts, self.out, self.ind, self.arm_tail) = saved;
                self.helpers.insert("$handle");
                let h = format!("(function* () {{\n{b}{pad}}}, [{}], function* ($op, $k) {{\n{arms_js}{pad}}})", ops.join(", "));
                if self.gen_ { format!("(yield $handle{h})") } else { format!("$run{h}") }
            }
        }
    }

    /// The comparator of `==` by content for values of type `t` (None: `===` is it). It compares the declared fields
    /// only, so it is symmetric and ignores properties a value from TS may carry beyond its type, as the wasm layer
    /// (which rebuilds the value from its fields) does; NaN is unequal to itself anywhere, as in Rust. A recursive
    /// type calls its own comparator
    fn eq_fn(&mut self, t: &Ty) -> Option<String> {
        use crate::check::subst_named;
        if let Some(f) = self.eqs.get(t) {
            return Some(f.clone());
        }
        let compound = match t {
            Ty::Array(_) | Ty::Typed(_) | Ty::Tuple(_) => true,
            Ty::Named(n, _) => self.info.records.contains_key(n) || self.info.enums.get(n).is_some_and(|e| !e.is_effect() && !self.string_enum(n)),
            _ => false,
        };
        if !compound {
            return None;
        }
        // (named before its body is written: a recursive type's comparator calls itself)
        let f = format!("$eq{}", self.eqs.len() + 1);
        self.eqs.insert(t.clone(), f.clone());
        let cmp = |js: &mut Js, t: &Ty, a: &str, b: &str| match js.eq_fn(t) {
            Some(g) => format!("{g}({a}, {b})"),
            None => format!("{a} === {b}"),
        };
        let body = match t {
            Ty::Array(e) | Ty::Typed(e) => {
                let c = cmp(self, e, "a[i]", "b[i]");
                format!("  if (a.length !== b.length) return false;\n  for (let i = 0; i < a.length; i++) if (!({c})) return false;\n  return true;\n")
            }
            Ty::Tuple(ts) => {
                let cs: Vec<String> = ts.iter().enumerate().map(|(k, t)| cmp(self, t, &format!("a[{k}]"), &format!("b[{k}]"))).collect();
                format!("  return {};\n", if cs.is_empty() { "true".into() } else { cs.join(" && ") })
            }
            Ty::Named(n, targs) if self.info.records.contains_key(n) => {
                let r = &self.info.records[n];
                let m: HashMap<String, Ty> = r.params.iter().cloned().zip(targs.iter().cloned()).collect();
                let fields: Vec<(String, Ty)> = r.fields.iter().map(|(f, t)| (f.clone(), subst_named(t, &m))).collect();
                let cs: Vec<String> = fields.iter().map(|(f, t)| cmp(self, t, &format!("a.{f}"), &format!("b.{f}"))).collect();
                format!("  return {};\n", if cs.is_empty() { "true".into() } else { cs.join(" && ") })
            }
            Ty::Named(n, targs) => {
                let e = &self.info.enums[n];
                let m: HashMap<String, Ty> = e.params.iter().cloned().zip(targs.iter().cloned()).collect();
                let vs: Vec<(String, Vec<Ty>)> = e.variants.iter().map(|(v, fs)| (v.clone(), fs.iter().map(|t| subst_named(t, &m)).collect())).collect();
                let key = self.tag_key(n);
                let n = n.clone();
                let mut s = format!("  if (a.{key} !== b.{key}) return false;\n  switch (a.{key}) {{\n");
                for (v, fs) in vs {
                    if fs.is_empty() {
                        continue;
                    }
                    let cs: Vec<String> = fs.iter().enumerate().map(|(k, t)| {
                        let f = self.field_key(&n, &v, k);
                        cmp(self, t, &format!("a.{f}"), &format!("b.{f}"))
                    }).collect();
                    let _ = writeln!(s, "    case {}: return {};", self.tag_js(&n, &v), cs.join(" && "));
                }
                s.push_str("  }\n  return true;\n");
                s
            }
            _ => unreachable!(),
        };
        let _ = write!(self.eq_defs, "function {f}(a, b) {{\n{body}}}\n\n");
        Some(f)
    }

    fn help(&mut self, h: &'static str, v: &str) -> String {
        self.helpers.insert(h);
        format!("{h}({v})")
    }

    /// A value JS gave (an extern's result, a property of an extern type), as zustscript's: `null` / `undefined`
    /// is `None`, and a `Result` catches what the call throws (`call` is evaluated inside the catch)
    fn js_result(&mut self, call: String, r: &Ty) -> String {
        let message = |e: &Ty| if matches!(e, Ty::Str) { "true" } else { "false" };
        match r {
            Ty::Named(n, a) if n == "Option" && a.len() == 1 => {
                self.helpers.insert("$nullable");
                format!("$nullable({call})")
            }
            Ty::Named(n, a) if n == "Result" && a.len() == 2 => {
                let inner = self.js_result(call, &a[0]);
                self.helpers.insert("$catch");
                format!("$catch(() => {inner}, {})", message(&a[1]))
            }
            Ty::Named(n, a) if n == "Promise" && a.len() == 1 => match &a[0] {
                Ty::Named(m, b) if m == "Result" && b.len() == 2 => {
                    let inner = if is_option(&b[0]) {
                        self.helpers.insert("$nullable");
                        format!("{call}.then($nullable)")
                    } else {
                        call
                    };
                    self.helpers.insert("$catchP");
                    format!("$catchP(() => {inner}, {})", message(&b[1]))
                }
                t if is_option(t) => {
                    self.helpers.insert("$nullable");
                    format!("{call}.then($nullable)")
                }
                _ => call,
            },
            _ => call,
        }
    }

    /// A call of JS (`make` writes it from the values of its receiver and arguments), its result converted. When the
    /// result catches what the call throws, the values are evaluated before, where the call stands (an `await` or a
    /// `yield` in one stays in the function being written): `(($v0) => $catch(() => f($v0), true))(await p)`
    fn extern_call(&mut self, vals: Vec<String>, ret: &Ty, make: impl Fn(&[String]) -> String) -> String {
        let catches = match ret {
            Ty::Named(n, a) if n == "Result" && a.len() == 2 => true,
            Ty::Named(n, a) if n == "Promise" && a.len() == 1 => matches!(&a[0], Ty::Named(m, b) if m == "Result" && b.len() == 2),
            _ => false,
        };
        // (a property read may run a getter that throws: it is evaluated outside the catch too)
        let plain = |s: &String| s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') || (s.len() >= 2 && s.starts_with('"') && s.ends_with('"') && !s[1..s.len() - 1].contains('"'));
        if !catches || vals.iter().all(plain) {
            return self.js_result(make(&vals), ret);
        }
        let ps: Vec<String> = (0..vals.len()).map(|k| format!("$v{k}")).collect();
        let inner = self.js_result(make(&ps), ret);
        format!("(({}) => {inner})({})", ps.join(", "), vals.join(", "))
    }

    /// A value handed to JS (an extern's argument): `None` is `undefined`
    fn js_arg(&mut self, v: String, t: &Ty) -> String {
        if is_option(t) {
            // (a literal: its JS value as it is)
            if v == "{ TAG: \"None\" }" {
                return "undefined".into();
            }
            if let Some(x) = v.strip_prefix("{ TAG: \"Some\", _0: ").and_then(|x| x.strip_suffix(" }"))
                && balanced(x)
            {
                return x.to_string();
            }
            self.helpers.insert("$undef");
            return format!("$undef({v})");
        }
        v
    }

    fn note_extern(&mut self, n: &str) {
        if let Some((_, Some(m), js)) = self.info.externs.get(n) {
            self.imports.entry(m.clone()).or_default().insert(js_root(js).to_string());
        }
    }

    fn binary(&mut self, id: Id, op: &str, l: &Expr, r: &Expr) -> String {
        // (values with parts compare by content, as Rust's derive(PartialEq) does in the wasm layer: a comparator
        // written for the type)
        if let Some(t) = self.info.deep_eq.get(&id).cloned() {
            let t = subst_ty(&t, &self.subst);
            if let Some(f) = self.eq_fn(&t) {
                let a = self.expr(l);
                let a = self.pin_before(l, a, &[r]);
                let b = self.expr(r);
                return if op == "==" { format!("{f}({a}, {b})") } else { format!("!{f}({a}, {b})") };
            }
        }
        let t = self.num(id);
        // (operands of the wrapping forms are inside `(..)` or a call: any precedence above `|` and `,` will do)
        let p = match (&t, op) {
            (Ty::I32 | Ty::U32 | Ty::I64, "+" | "-" | "*" | "/" | "%") => if op == "+" || op == "-" { bin_prec(op) } else { 3 },
            (Ty::F32, "+" | "-" | "*" | "/" | "%") => bin_prec(op),
            _ => bin_prec(op),
        };
        let a = self.bin_operand(l, p, false);
        let a = self.pin_before(l, a, &[r]);
        let b = self.bin_operand(r, p, true);
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
            // (a component imported from another module is imported in the JS)
            self.note_extern(&j.tag);
            match self.info.externs.get(&j.tag) {
                // (an extern's JS may be another name, `UI.Badge`)
                Some((_, _, js)) => js.clone(),
                None => ident(&j.tag),
            }
        } else {
            js_str(&j.tag)
        };
        let mut props = vec![];
        let mut key = None;
        // (the attributes, then the children, in order)
        let mut all: Vec<&Expr> = j.attrs.iter().map(|a| &a.1).collect();
        all.extend(j.children.iter().filter_map(|c| match c {
            JsxChild::Expr(x) => Some(x),
            JsxChild::Text(_) => None,
        }));
        let mut vals = self.seq(&all).into_iter();
        for (a, _) in &j.attrs {
            let v = vals.next().unwrap();
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
                JsxChild::Expr(_) => vals.next().unwrap(),
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
            '"' | '\'' => quote = Some(c),
            // (a template's text and holes nest (`${`(`}`): not scanned here. Not a single call, so the caller
            // keeps its parentheses)
            '`' => return false,
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

/// The name an extern's JS starts from: `api` of `api.read`, `URL` of `new URL` (what a module's extern imports)
fn js_root(js: &str) -> &str {
    let js = js.strip_prefix("new ").unwrap_or(js);
    js.split('.').next().unwrap_or(js)
}

/// `(x)` → `x` (an operand written inside a call's parentheses needs none of its own)
fn unparen(s: &str) -> &str {
    match s.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(x) if balanced(x) => x,
        _ => s,
    }
}

fn is_option(t: &Ty) -> bool {
    matches!(t, Ty::Named(n, a) if n == "Option" && a.len() == 1)
}

/// Does a value of `t` from JS need converting ([`Js::js_result`])
fn needs_js_result(t: &Ty) -> bool {
    match t {
        Ty::Named(n, a) if (n == "Option" && a.len() == 1) || (n == "Result" && a.len() == 2) => true,
        Ty::Named(n, a) if n == "Promise" && a.len() == 1 => needs_js_result(&a[0]),
        _ => false,
    }
}

/// `.name` (or `["data-x"]` for a JS name that is not an identifier)
fn member(js: &str) -> String {
    if !js.is_empty() && js.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$') && !js.starts_with(|c: char| c.is_ascii_digit()) { format!(".{js}") } else { format!("[{}]", js_str(js)) }
}

/// An assignment of a local in `e` (not in a closure: one cannot assign a local outside it)
fn assigns_local(e: &Expr) -> bool {
    match &e.kind {
        EK::Assign(..) => true,
        EK::Closure(..) | EK::Async(_) => false,
        _ => crate::check::subexprs(e).into_iter().any(assigns_local),
    }
}

/// A `?` in `e` that is not inside a closure of its own (it returns from the function `e` is in)
pub fn has_try(e: &Expr) -> bool {
    match &e.kind {
        EK::Try(_) => true,
        EK::Closure(..) | EK::Async(_) => false,
        _ => crate::check::subexprs(e).into_iter().any(has_try),
    }
}

/// An `await` in `e` that is not inside a closure of its own (it belongs to the function `e` is in)
fn awaits(e: &Expr) -> bool {
    match &e.kind {
        EK::Await(_) => true,
        EK::Closure(..) | EK::Async(_) => false,
        _ => crate::check::subexprs(e).into_iter().any(awaits),
    }
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
            EK::Assign(_, x) | EK::Un(_, x) | EK::Field(x, _) | EK::Cast(x, _) | EK::Try(x) | EK::Await(x) => walk(x),
            EK::Bin(_, a, b) | EK::Index(a, b) => walk(a).or_else(|| walk(b)),
            EK::Call(f, a) => walk(f).or_else(|| a.iter().find_map(walk)),
            EK::Method(r, _, a) => walk(r).or_else(|| a.iter().find_map(walk)),
            EK::Array(xs) | EK::Tuple(xs) | EK::Interp(_, xs) => xs.iter().find_map(walk),
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
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in s.chars() {
        // (parentheses inside a string literal are not structure: `(yield k("(")) + h(")")` is not one group)
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
            '"' | '\'' => quote = Some(c),
            // (a template's text and holes nest (`${`(`}`): not scanned here. Not known to be balanced, so the
            // caller keeps the parentheses and the group as they are)
            '`' => return false,
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

// ---------------------------------------------------------------- the extern check (TS declarations vs zustscript)

/// A zustscript type as TS sees the JS value: numbers are `number` (no brands: TS's own declarations use `number`)
fn ts_plain(t: &Ty, named: &mut BTreeSet<String>) -> String {
    match t {
        Ty::I32 | Ty::U32 | Ty::F32 | Ty::F64 => "number".into(),
        Ty::I64 => "bigint".into(),
        Ty::Str => "string".into(),
        Ty::Bool => "boolean".into(),
        Ty::Unit => "void".into(),
        Ty::Typed(e) => typed_js(e).into(),
        Ty::Jsx => {
            named.insert("$ReactElement".into());
            "$ReactElement".into()
        }
        Ty::Array(a) => {
            let s = ts_plain(a, named);
            if matches!(**a, Ty::Fn(..)) { format!("readonly ({s})[]") } else { format!("readonly {s}[]") }
        }
        Ty::Tuple(ts_) => format!("readonly [{}]", ts_.iter().map(|t| ts_plain(t, named)).collect::<Vec<_>>().join(", ")),
        Ty::Fn(a, r) => {
            let ps: Vec<String> = a.iter().enumerate().map(|(k, t)| format!("a{k}: {}", ts_plain(t, named))).collect();
            format!("({}) => {}", ps.join(", "), ts_plain(r, named))
        }
        // (a local copy of the record or enum with its numbers unbranded: TS's values carry no brands)
        Ty::Named(n, a) => {
            named.insert(n.clone());
            if a.is_empty() { format!("$P_{n}") } else { format!("$P_{n}<{}>", a.iter().map(|t| ts_plain(t, named)).collect::<Vec<_>>().join(", ")) }
        }
        Ty::Param(p) => p.clone(),
        Ty::Var(_) => "unknown".into(),
    }
}

/// A type as JS hands it to (or takes it from) an extern: `T | null | undefined` for an `Option` result (`T |
/// undefined` for an argument), `T` for a `Result` (what does not throw), and so in a `Promise`
fn ts_js_side(t: &Ty, named: &mut BTreeSet<String>, ret: bool) -> String {
    match t {
        Ty::Named(n, a) if n == "Option" && a.len() == 1 => {
            let t = ts_plain(&a[0], named);
            let t = if matches!(a[0], Ty::Fn(..)) { format!("({t})") } else { t };
            format!("{t} | {}undefined", if ret { "null | " } else { "" })
        }
        Ty::Named(n, a) if ret && n == "Result" && a.len() == 2 => ts_js_side(&a[0], named, ret),
        Ty::Named(n, a) if ret && n == "Promise" && a.len() == 1 => format!("Promise<{}>", ts_js_side(&a[0], named, ret)),
        t => ts_plain(t, named),
    }
}

/// The unbranded copy of a record or enum, in the `.d.ts`'s shape
fn ts_plain_def(n: &str, items: &[Item], info: &Info, named: &mut BTreeSet<String>) -> Option<String> {
    for it in items {
        match it {
            // (JS's own type, by its TS name)
            Item::ExternType { name, module, params, ts, .. } if name == n => {
                let g = if params.is_empty() { String::new() } else { format!("<{}>", params.join(", ")) };
                return Some(match module {
                    Some(m) => format!("import type {{ {ts} as $I_{n} }} from {};\ntype $P_{n}{g} = $I_{n}{g};", js_str(m)),
                    None => format!("type $P_{n}{g} = {ts}{g};"),
                });
            }
            Item::Record { name, params, .. } if name == n => {
                let g = if params.is_empty() { String::new() } else { format!("<{}>", params.join(", ")) };
                let fs: Vec<String> = info.records[n].fields.iter().map(|(f, t)| format!("readonly {f}: {}", ts_plain(t, named))).collect();
                return Some(format!("type $P_{n}{g} = {{ {} }};", fs.join("; ")));
            }
            Item::Enum { name, params, .. } if name == n => {
                let e = &info.enums[n];
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
                                let _ = write!(s, "; readonly _{k}: {}", ts_plain(t, named));
                            }
                            s + " }"
                        }
                    })
                    .collect();
                return Some(format!("type $P_{n}{g} = {};", alts.join(" | ")));
            }
            _ => {}
        }
    }
    None
}

/// The TS file that checks every `extern` against the TS declaration it names: the real function must be
/// assignable to the type zustscript gives it (arguments are checked contravariantly, results covariantly).
/// Returns the file and, for each extern, (its line in the file, the extern's name, its position in the source).
pub fn extern_check(items: &[Item], info: &Info) -> Option<(String, Vec<(usize, String, Pos)>)> {
    let mut named = BTreeSet::new();
    let mut imports: Vec<String> = vec![];
    let mut checks: Vec<(String, String, Pos)> = vec![];
    for it in items {
        if let Item::Extern { name, module, pos, .. } = it {
            let (sig, _, js) = &info.externs[name];
            let g = if sig.generics.is_empty() { String::new() } else { format!("<{}>", sig.generics.join(", ")) };
            let side = |t: &Ty, named: &mut BTreeSet<String>, ret: bool| if info.zsc_externs.contains(name) { ts_plain(t, named) } else { ts_js_side(t, named, ret) };
            let ps: Vec<String> = sig.args.iter().enumerate().map(|(k, t)| format!("a{k}: {}", side(t, &mut named, false))).collect();
            let want = format!("{g}({}) => {}", ps.join(", "), side(&sig.ret, &mut named, true));
            let k = checks.len();
            let value = match module {
                // (a constructor: called with `new`, its parameters the class's)
                _ if js.starts_with("new ") => {
                    let c = &js[4..];
                    let c = match module {
                        Some(m) => {
                            let root = js_root(js);
                            imports.push(format!("import {{ {root} as $e{k} }} from {};", js_str(m)));
                            format!("$e{k}{}", &c[root.len()..])
                        }
                        None => c.to_string(),
                    };
                    format!("(...a: ConstructorParameters<typeof {c}>) => new {c}(...a)")
                }
                Some(m) => {
                    // (a dotted name `api.read`: import the root, reach the member through it)
                    let root = js.split('.').next().unwrap_or(js);
                    imports.push(format!("import {{ {root} as $e{k} }} from {};", js_str(m)));
                    format!("$e{k}{}", &js[root.len()..])
                }
                None => js.clone(),
            };
            checks.push((format!("const $c{k}: {want} = {value};"), name.clone(), *pos));
        }
        // (each member of an extern type: read or called on a value of the TS type, it fits the declared type)
        if let Item::ExternType { name, params, members, .. } = it {
            let x = &info.ext_types[name];
            let at = |n: &str| members.iter().find_map(|m| match m {
                Member::Prop { name, pos, .. } | Member::Method { name, pos, .. } if name == n => Some(*pos),
                _ => None,
            }).unwrap_or_default();
            let g = if params.is_empty() { String::new() } else { format!("<{}>", params.join(", ")) };
            let o = format!("o: $P_{name}{g}");
            named.insert(name.clone());
            for (p, t, js) in &x.props {
                let k = checks.len();
                let want = format!("{g}({o}) => {}", ts_js_side(t, &mut named, true));
                checks.push((format!("const $c{k}: {want} = (o) => o{};", member(js)), format!("{name}.{p}"), at(p)));
            }
            for (m, args, ret, js) in &x.methods {
                let k = checks.len();
                let ps: Vec<String> = args.iter().enumerate().map(|(k, t)| format!("a{k}: {}", ts_js_side(t, &mut named, false))).collect();
                let c = match js.strip_suffix('=') {
                    Some(p) => format!("const $c{k}: {g}({o}, {}) => void = (o, v) => {{ o{} = v; }};", ps.join(", "), member(p)),
                    None => format!("const $c{k}: {g}({}) => {} = (o, ...a) => o{}(...a);", [o.clone()].into_iter().chain(ps).collect::<Vec<_>>().join(", "), ts_js_side(ret, &mut named, true), member(js)),
                };
                checks.push((c, format!("{name}.{m}"), at(m)));
            }
        }
    }
    if checks.is_empty() {
        return None;
    }
    let mut o = String::from("// generated by zsc --check-externs: each extern's TS declaration must fit its zustscript signature\n");
    let mut defs = vec![];
    let mut done = BTreeSet::new();
    done.insert("$ReactElement".to_string());
    while let Some(n) = named.iter().find(|n| !done.contains(*n)).cloned() {
        done.insert(n.clone());
        if let Some(d) = ts_plain_def(&n, items, info, &mut named) {
            defs.push(d);
        }
    }
    // (after the expansion: a record's Jsx field needs the import too)
    if named.contains("$ReactElement") {
        o.push_str("import type { ReactElement as $ReactElement } from \"react\";\n");
    }
    for i in imports {
        o.push_str(&i);
        o.push('\n');
    }
    o.push_str("export {};\n");
    for d in defs {
        o.push_str(&d);
        o.push('\n');
    }
    let mut lines = vec![];
    for (c, name, pos) in checks {
        let _ = writeln!(o, "// extern {name}");
        lines.push((o.lines().count() + 1, name, pos));
        o.push_str(&c);
        o.push('\n');
    }
    Some((o, lines))
}

/// The name of the shared runtime module (`zsc-runtime.js` and `.d.ts`, written next to the modules)
pub const RUNTIME: &str = "zsc-runtime";
/// The first line of the shared runtime file: how zsc tells it from a module of the same name.
pub const RUNTIME_HEADER: &str = "// generated by zsc: the runtime the modules share (helpers, and the constructors of the numeric brands)\n";

/// The shared runtime: every helper and every brand constructor, exported (a bundler keeps only what the modules
/// import). Its content does not depend on the module, so the modules compiled into one directory share one file.
pub fn runtime_js() -> String {
    let mut o = String::from(RUNTIME_HEADER);
    o.push('\n');
    for (_, code) in HELPERS {
        for line in code.lines() {
            if line.starts_with("function") || line.starts_with("async function") {
                o.push_str("export ");
            }
            o.push_str(line);
            o.push('\n');
        }
        o.push('\n');
    }
    let all: Vec<&str> = BRANDS.iter().map(|b| b.0).collect();
    o + &brand_runtime(&all)
}

/// The types of the shared runtime: the numeric brands and their constructors
pub fn runtime_dts() -> String {
    let mut o = String::from("// generated by zsc: the numeric brands (a brand on top of number; make one with `i32(1)` / `f64(0.5)`)\n\n");
    for (n, what, _) in BRANDS {
        let _ = writeln!(o, "/** {what} */\nexport type {n} = number & {{ readonly __zsc_num: \"{n}\" }};\nexport declare function {n}(x: number): {n};");
    }
    o
}

/// With the shared runtime, the module re-exports the brand constructors its `.d.ts` uses (TS code imports them
/// from the module, as before)
pub fn brand_reexport(used: &[&str]) -> String {
    if used.is_empty() {
        return String::new();
    }
    format!("export {{ {} }} from \"./{RUNTIME}.js\";\n", used.join(", "))
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

/// An enum's type in the `.d.ts`: a union of string literals, or of `{ TAG, _0, .. }` objects
fn enum_dts(name: &str, params: &[String], info: &Info) -> String {
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
    format!("export type {name}{g} = {};\n\n", alts.join(" | "))
}

/// The `.d.ts`, and the numeric brands it uses
/// (`spec`: the module specifier of a TS file a record was read from, from the `.d.ts`'s directory)
pub fn dts(items: &[Item], info: &Info, shared_runtime: bool, spec: &dyn Fn(&std::path::Path) -> String) -> (String, Vec<&'static str>) {
    let mut o = String::new();
    USES_JSX.with(|u| u.set(false));
    // (a record read from a TS file is that file's type: imported, not declared again)
    let mut ts_imports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for it in items {
        match it {
            Item::Record { from: Some(f), name, .. } | Item::Enum { from: Some(f), name, .. } if f.exported => {
                ts_imports.entry(spec(&f.path)).or_default().insert(name.clone());
            }
            // (a union the file does not export: declared again as the file writes it)
            Item::Enum { from: Some(f), name, .. } => {
                let _ = writeln!(o, "export type {name} = {};\n", f.text);
            }
            // (a type the file does not export, read because an imported one uses it: declared again with the TS
            // file's own members, so it is the same type to TS)
            Item::Record { from: Some(f), name, .. } => {
                let _ = writeln!(o, "export interface {name} {{");
                for m in &f.members {
                    let _ = writeln!(o, "  {m};");
                }
                o.push_str("}\n\n");
            }
            Item::Record { name, params, .. } => {
                let r = &info.records[name];
                let g = if params.is_empty() { String::new() } else { format!("<{}>", params.join(", ")) };
                let _ = writeln!(o, "export interface {name}{g} {{");
                for (f, t) in &r.fields {
                    let _ = writeln!(o, "  readonly {f}: {};", ts(t));
                }
                o.push_str("}\n\n");
            }
            // (the prelude's: declared below, when the declarations use it)
            Item::Enum { pos, .. } if crate::ast::is_prelude(*pos) => {}
            Item::Enum { name, params, .. } => o.push_str(&enum_dts(name, params, info)),
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
    // (`Result<T, E>` may hold an `Option`: the prelude's types that are used, the last first)
    for it in items.iter().rev() {
        if let Item::Enum { name, params, pos, .. } = it
            && crate::ast::is_prelude(*pos)
            && uses_word(&o, name)
        {
            o = enum_dts(name, params, info) + &o;
        }
    }
    // (an extern type is TS's: imported from its module, or named as TS names it)
    let mut ext_imports = String::new();
    for it in items {
        if let Item::ExternType { name, module, params, ts, .. } = it
            && uses_word(&o, name)
        {
            let g = if params.is_empty() { String::new() } else { format!("<{}>", params.join(", ")) };
            match module {
                Some(m) if ts == name => {
                    let _ = writeln!(ext_imports, "import type {{ {name} }} from {};", js_str(m));
                }
                Some(m) => {
                    let _ = writeln!(ext_imports, "import type {{ {ts} as {name} }} from {};", js_str(m));
                }
                None if ts != name => o = format!("/** JS's `{ts}` */\ntype {name}{g} = {ts}{g};\n\n") + &o,
                None => {}
            }
        }
    }
    let used: Vec<&'static str> = BRANDS.iter().map(|b| b.0).filter(|n| uses_word(&o, n)).collect();
    let mut h = String::from("// generated by zsc: the types of the module's exports\n");
    h.push_str(&ext_imports);
    if USES_JSX.with(|u| u.replace(false)) {
        h.push_str("import type { ReactElement as $ReactElement } from \"react\";\n");
    }
    for (m, names) in &ts_imports {
        // (only the types the declarations name)
        let names: Vec<String> = names.iter().filter(|n| uses_word(&o, n)).cloned().collect();
        if !names.is_empty() {
            let _ = writeln!(h, "import type {{ {} }} from {};", names.join(", "), js_str(m));
        }
    }
    h.push('\n');
    if !used.is_empty() && shared_runtime {
        let _ = writeln!(h, "// numbers keep their zustscript type: a brand on top of number (make one with `i32(1)` / `f64(0.5)`)\nimport {{ {0} }} from \"./{RUNTIME}.js\";\nexport {{ {0} }};\n", used.join(", "));
    } else if !used.is_empty() {
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
