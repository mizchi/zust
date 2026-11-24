//! The prototype's tests: examples/*.zsc compile and run under node with the expected output, the output passes
//! `node --check`, and tests/errors/*.zsc fail with the message their first line expects.
//! (app.zsc needs zustc: built in the zust checkout this crate sits in, `cargo build --release` there)

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn zsc(input: &Path, out: &Path, flags: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zsc")).arg(input).arg("-o").arg(out).args(flags).output().unwrap()
}

fn out_dir(name: &str) -> PathBuf {
    let d = root().join("target/test-out").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("node_modules")).unwrap();
    // (the JS side of the helpers, as a package: the glue imports `zustscript-helper`)
    let helper = d.join("node_modules/zustscript-helper");
    std::fs::create_dir_all(&helper).unwrap();
    for f in ["package.json", "index.js", "index.d.ts", "bytes.js", "bytes.d.ts"] {
        std::fs::copy(root().join("helper/ts").join(f), helper.join(f)).unwrap();
    }
    // (a stand-in for React: renders to a string)
    let stub = root().join("tests/stub/react");
    let dst = d.join("node_modules/react");
    std::fs::create_dir_all(&dst).unwrap();
    for f in std::fs::read_dir(&stub).unwrap() {
        let f = f.unwrap().path();
        std::fs::copy(&f, dst.join(f.file_name().unwrap())).unwrap();
    }
    d
}

fn node(dir: &Path, script: &str) -> String {
    let o = Command::new("node").current_dir(dir).arg("--input-type=module").arg("-e").arg(script).output().expect("node");
    assert!(o.status.success(), "node failed:\n{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

fn build(name: &str) -> PathBuf {
    let d = out_dir(name);
    let o = zsc(&root().join(format!("examples/{name}.zsc")), &d, &[]);
    assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
    let c = Command::new("node").arg("--check").arg(d.join(format!("{name}.js"))).output().unwrap();
    assert!(c.status.success(), "the output is not valid JS:\n{}", String::from_utf8_lossy(&c.stderr));
    d
}

#[test]
fn export_visibility() {
    let d = out_dir("export_visibility");
    let src = d.join("visibility.zsc");
    std::fs::write(&src, "fn hidden() -> i32 { 1 }\nexport fn current() -> i32 { hidden() }\npub fn legacy() -> i32 { current() }").unwrap();
    let o = zsc(&src, &d, &[]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = node(&d, "const m = await import('./visibility.js'); console.log(m.current(), m.legacy(), 'hidden' in m);");
    assert_eq!(out.trim(), "1 1 false");
    let dts = std::fs::read_to_string(d.join("visibility.d.ts")).unwrap();
    assert!(dts.contains("export declare function current(") && dts.contains("export declare function legacy("));
    assert!(!dts.contains("hidden"));
}

#[test]
fn basics() {
    let d = build("basics");
    let out = node(&d, "const m = await import('./basics.js'); m.main();");
    assert_eq!(
        out,
        "areas: 12.566370614359172, 12\n\
         active: type check, emit JS\n\
         toggled: false,true,true\n\
         found type check / none\n\
         i32: -2147483648 3 -1\n\
         i64: 27021597764222979\n\
         u32: 4294967295\n\
         sums: 6 3.75 2.3333333333333335\n\
         casts: 3 2147483647 2.5\n"
    );
    let dts = std::fs::read_to_string(d.join("basics.d.ts")).unwrap();
    assert!(dts.contains("export type Filter = \"All\" | \"Active\" | \"Done\";"));
    assert!(dts.contains("export declare function sum$i64(xs: readonly bigint[]): bigint;"));
    assert!(dts.contains("export declare function sum$i32(xs: readonly i32[]): i32;"));
    // (the brands are declared once, in the shared runtime, and re-exported by the module)
    assert!(dts.contains("import { i32, f64 } from \"./zsc-runtime.js\";") || dts.contains("from \"./zsc-runtime.js\";"), "{dts}");
    let rt = std::fs::read_to_string(d.join("zsc-runtime.d.ts")).unwrap();
    assert!(rt.contains("export type i32 = number & { readonly __zsc_num: \"i32\" };"));
    let js = std::fs::read_to_string(d.join("basics.js")).unwrap();
    assert!(!js.contains("class "), "the output uses no classes");
    // the brands' constructors check at the boundary
    let out = node(&d, "const m = await import('./basics.js'); console.log(m.i32(3)); try { m.i32(1.5) } catch (e) { console.log(e.message) }");
    assert_eq!(out, "3\nnot a 32-bit signed integer: 1.5\n");
}

#[test]
fn effects() {
    let d = build("effects");
    let out = node(&d, "const m = await import('./effects.js'); m.main();");
    assert_eq!(
        out,
        "[ask] name?\n[out] hello, taro\ngreeted taro\n[state] put 11\n[state] put 11\n[state] put 11\ncounter: 10\n\
         raised: divide by zero\nresults: -1 3\n"
    );
    // State and Console have only tail-resumptive handlers: plain calls. Fail's handler does not resume: a generator
    let js = std::fs::read_to_string(d.join("effects.js")).unwrap();
    assert!(js.contains("$perform(\"Get\")") && js.contains("$handleD("), "{js}");
    assert!(js.contains("function* safe_div") && !js.contains("function* counter"), "{js}");
}

#[test]
fn app_with_wasm() {
    let d = build("app");
    let out = node(
        &d,
        r#"
const m = await import('./app.js');
const { render, jsx } = await import('react/jsx-runtime');
console.log(m.demo());
console.log(JSON.stringify(m.stats({ label: "a", values: [1, 2, 3, 4] })));
console.log("callbacks left:", m.$cbs.size);
console.log(render(jsx(m.App, { samples: [{ label: "b", values: [0.5, 0.25] }] })));
let picked = null;
m.Row({ sample: { label: "c", values: [1] }, onPick: (l) => { picked = l; } }).props.onClick();
console.log("picked", picked);
"#,
    );
    assert_eq!(
        out,
        "map_sum 30, call_twice 63, HéLLO WöRLD!\n\
         {\"mean\":2.5,\"stddev\":1.118033988749895,\"max\":4}\n\
         callbacks left: 0\n\
         <h1>SAMPLES!</h1><ul><li data-on=\"onClick\"><b>b</b> mean 0.375, stddev 0.125<div><span class=\"bar\" style=\"width:10px\">1</span><span class=\"bar\" style=\"width:0px\">0</span><span class=\"bar\" style=\"width:0px\">0</span><span class=\"bar\" style=\"width:10px\">1</span></div></li></ul><p>picked: none</p>\n\
         picked c\n"
    );
}

#[test]
fn errors() {
    let dir = root().join("tests/errors");
    let mut n = 0;
    for f in std::fs::read_dir(&dir).unwrap() {
        let f = f.unwrap().path();
        let src = std::fs::read_to_string(&f).unwrap();
        let want = src.lines().next().unwrap().strip_prefix("// expect: ").expect("first line: // expect: <message>");
        let o = zsc(&f, &root().join("target/test-out/errors"), &[]);
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{} compiled", f.display());
        assert!(err.contains(want), "{}: expected `{want}` in:\n{err}", f.display());
        n += 1;
    }
    assert!(n >= 10);
}

/// The .d.ts files type-check from TS, and the numeric brands reject mixing (tests/consumer.ts). Needs tsc: the
/// `TSC` variable, or `tsc` on PATH; skipped otherwise.
#[test]
fn dts_typechecks() {
    let tsc = std::env::var("TSC").unwrap_or_else(|_| "tsc".into());
    if Command::new(&tsc).arg("--version").output().is_err() {
        eprintln!("skipped: no tsc (set TSC)");
        return;
    }
    let d = out_dir("ts");
    for name in ["basics", "app", "ml"] {
        let o = zsc(&root().join(format!("examples/{name}.zsc")), &d, &[]);
        assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
    }
    std::fs::copy(root().join("tests/consumer.ts"), d.join("consumer.ts")).unwrap();
    // (zsc's output is ES modules)
    std::fs::write(d.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
    let o = Command::new(&tsc)
        .current_dir(&d)
        .args(["--strict", "--noEmit", "--target", "es2022", "--module", "nodenext", "--moduleResolution", "nodenext", "consumer.ts"])
        .output()
        .unwrap();
    assert!(o.status.success(), "tsc:\n{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
}

// The JS control uses the same bodies as ordinary functions. The annotated fixture exercises the Wasm target.
fn compare_build(src: &Path, out: &Path, flags: &[&str]) -> std::process::Output {
    if !flags.contains(&"--all-js") {
        return zsc(src, out, flags);
    }
    let plain = out.join(src.file_name().unwrap());
    let source = std::fs::read_to_string(src).unwrap();
    let lines: Vec<_> = source.lines().map(|line| line.strip_prefix("@wasm ").unwrap_or(line)).collect();
    std::fs::write(&plain, lines.join("\n")).unwrap();
    zsc(&plain, out, flags)
}

/// examples/numeric.zsc's `@wasm fn`s give the same results lowered to Rust and run as wasm, and run as JS
/// (`--all-js`): numbers keep their meaning in both (i32 / u32 wrapping, i64 past 2^53, saturating `as`, errors)
#[test]
fn numeric_same_in_js_and_wasm() {
    let src = root().join("examples/numeric.zsc");
    let mut outs = vec![];
    for (name, flags) in [("numeric-wasm", &[][..]), ("numeric-js", &["--all-js"][..])] {
        let d = out_dir(name);
        let o = compare_build(&src, &d, flags);
        assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(d.join("numeric.wasm").exists(), flags.is_empty());
        let r = Command::new("node").arg(root().join("tests/numeric_driver.mjs")).arg(d.join("numeric.js")).output().unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        outs.push(String::from_utf8(r.stdout).unwrap());
    }
    assert_eq!(outs[0], outs[1], "wasm and JS differ");
    // an argument that throws while it is copied in (an array element's getter, in `.set`) gives back what the call
    // took before it: many such calls with large arrays leave the module's memory as it was
    let leak = node(
        &root().join("target/test-out/numeric-wasm"),
        "let ex; const inst = WebAssembly.instantiate; WebAssembly.instantiate = async (...a) => { const r = await inst(...a); ex = (r.instance ?? r).exports; return r; };
const m = await import('./numeric.js');
const big = new Array(200000).fill(1.5);
const bad = new Array(200000).fill(1); Object.defineProperty(bad, 7, { get() { throw new Error('getter'); } });
m.dot([1], [1]);
const before = ex.memory.buffer.byteLength;
let thrown = 0;
for (let i = 0; i < 200; i++) { try { m.dot(big, bad); } catch (e) { thrown++; } }
console.log(thrown, ex.memory.buffer.byteLength - before < 8 << 20, m.dot([2], [3]));",
    );
    assert_eq!(leak, "200 true 6\n");
    assert_eq!(
        outs[0],
        "dot 33.5\ntotal$i32 -2147483643\ntotal$u32 1\ntotal$i64 18014398509481986n\ntotal$f64 0.30000000000000004\n\
         mean 2.3333333333333335\nmean0 0\nnormalize [0.25,-1,0.5]\nnormalize0 [0,0]\nevens_squared [4,16,-2147386332,36]\n\
         escape [50,2,33]\ncollatz [111,410]\nlcg 350778849\nedge_cases [473956351,2147418112,0]\nout_of_bounds error\n"
    );
}

/// examples/shapes.zsc: records, enums (a generic one too), loops and mutable locals in `@wasm fn`s give the same
/// results as wasm and as JS (records and enums cross the boundary as their scalar slots)
#[test]
fn shapes_same_in_js_and_wasm() {
    let src = root().join("examples/shapes.zsc");
    let mut outs = vec![];
    for (name, flags) in [("shapes-wasm", &[][..]), ("shapes-js", &["--all-js"][..])] {
        let d = out_dir(name);
        let o = compare_build(&src, &d, flags);
        assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
        let c = Command::new("node").arg("--check").arg(d.join("shapes.js")).output().unwrap();
        assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
        let r = Command::new("node").arg(root().join("tests/shapes_driver.mjs")).arg(d.join("shapes.js")).output().unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        outs.push(String::from_utf8(r.stdout).unwrap());
    }
    assert_eq!(outs[0], outs[1], "wasm and JS differ");
    assert_eq!(
        outs[0],
        r#"area [28.274333882308138,8,0]
center [{"x":1,"y":2},{"x":2,"y":-1},{"x":5,"y":5}]
grow [{"TAG":"Circle","_0":{"x":1,"y":2},"_1":6},{"TAG":"Rect","_0":{"x":0,"y":0},"_1":{"x":8,"y":-4}},{"TAG":"Circle","_0":{"x":5,"y":5},"_1":2}]
nearest [{"TAG":"Some","_0":{"index":3,"dist":0.7071067811865476,"inside":true}},{"TAG":"Some","_0":{"index":2,"dist":9,"inside":false}},{"TAG":"None"}]
collatz_max [{"TAG":"Some","_0":"6171n"},{"TAG":"None"}]
"#
    );
}

/// examples/owned.zsc and tests/owned_cases.zsc: values that are not Copy (arrays inside records and enums) in
/// `@wasm`. The Rust's `&`, `'a`, moves and clones are inferred (src/lower.rs, "Ownership"); zustc's borrow checker
/// accepts it, and the results are the JS layer's
#[test]
fn owned_same_in_js_and_wasm() {
    let run = |src: &str, stem: &str, call: &str| -> (String, String) {
        let mut outs = vec![];
        let mut rust = String::new();
        for (name, flags) in [(format!("{stem}-wasm"), &[][..]), (format!("{stem}-js"), &["--all-js"][..])] {
            let d = out_dir(&name);
            let o = compare_build(&root().join(src), &d, flags);
            assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
            outs.push(node(&d, &format!("const m = await import('./{stem}.js'); console.log(JSON.stringify({call}))")));
            if flags.is_empty() {
                rust = std::fs::read_to_string(d.join(format!("{stem}.wasm.rs"))).unwrap();
            }
        }
        assert_eq!(outs[0], outs[1], "{src}: wasm and JS differ");
        (outs.remove(0), rust)
    };
    let (out, rs) = run("examples/owned.zsc", "owned", "[m.summary(6), m.ramp_sum(1000), m.appended([1, 2], 3), m.longer([1], [2, 3]), m.longer([4, 5], [6])]");
    assert_eq!(out, "[[346,90,30,1,20],499500,[1,2,3],[2,3],[4,5]]\n");
    // the inferred signatures: read only → borrowed; returned → borrowed through, with a lifetime; consumed → owned
    for sig in [
        "fn zs_total(p: &ZsPoly) -> f64",
        "fn zs_heavier<'a>(a: &'a ZsPoly, b: &'a ZsPoly) -> &'a ZsPoly",
        "fn zs_first_pts<'a>(ps: &'a [ZsPoly]) -> &'a [f64]",
        "fn zs_reweigh(p: ZsPoly, w: f64) -> ZsPoly",
        "fn zs_shape_area(s: &ZsShape) -> f64",
        "fn zs_appended(xs: Vec<f64>, x: f64) -> Vec<f64>",
        "fn zs_longer<'a>(a: &'a [f64], b: &'a [f64]) -> &'a [f64]",
    ] {
        assert!(rs.contains(sig), "missing `{sig}` in:\n{rs}");
    }
    // a record that holds an array is not Copy; one of scalars still is
    assert!(rs.contains("#[derive(Clone, PartialEq)]\nstruct ZsPoly"), "{rs}");
    // the push in the loop reuses the array (no copy of it), and the elements of a dead array are taken
    let ramp = &rs[rs.find("fn zs_ramp(").unwrap()..];
    let ramp = &ramp[..ramp.find("\n}\n").unwrap()];
    assert!(ramp.contains("let mut __zs_v = acc;") && !ramp.contains("to_vec"), "{ramp}");
    assert!(rs.contains("(zs_polys(n)).into_iter().map(|p|"), "{rs}");
    // a borrow of an owned local the rest of the block keeps: no copy
    assert!(rs.contains("let firsts = zs_first_pts(&ps[..]);"), "{rs}");
    // matched by reference through the borrowed parameter
    assert!(rs.contains("match *s {\nZsShape::Polygon(ref p, n)"), "{rs}");
    // the glue still passes slices and takes an owned result: adapters in front of the inferred signatures
    // (named in the generated `__zs` namespace: no source function's Rust name can be it)
    assert!(rs.contains("fn __zs_js_zs_appended(xs: &[f64], x: f64) -> Vec<f64> {\n    zs_appended(xs.to_vec(), x)\n}"), "{rs}");
    assert!(rs.contains("zs_longer(a, b).to_vec()"), "{rs}");

    let (out, rs) = run("tests/owned_cases.zsc", "owned_cases", "[m.cases(4), m.more(4), m.grow([5]), m.borrows(4)]");
    assert_eq!(out, "[[66.5,24,21.5,41.5,36,18.5,38,36],[65,10,5,4,9,6,4,13,6,2,6],[5,1],[12.5,7,12]]\n");
    // a borrow of an owned local kept by a `let`: the local is frozen (copied, not moved, while the borrow lives);
    // assigned later in the block, or leaving it, the borrow is copied instead
    assert!(rs.contains("let f = zs_id(&ys[..]);\nlet z = zs_keep(ys.to_vec());"), "{rs}");
    assert!(rs.contains("let f = zs_id(&ys[..]).to_vec();\nys = vec![1.0f64];"), "{rs}");
    assert!(rs.contains("let t = (zs_mk(n)).pts;\nzs_id(&t[..]).to_vec()"), "{rs}");
    // a result that borrows only its first parameter: a temporary second argument needs no copy
    assert!(rs.contains("fn zs_wrap<'a>(a: &'a [f64]) -> &'a [f64]"), "{rs}");
    assert!(rs.contains("fn zs_first<'a>(a: &'a [f64], b: &[f64]) -> &'a [f64]"), "{rs}");
    // a later argument assigns what an earlier one reads: the earlier one is taken as a value first
    assert!(rs.contains("zs_pair(&({ xs })[..], &({"), "{rs}");
    // the iterable is reassigned in the body: a dead one is taken, a borrowed one through a call is copied
    assert!(rs.contains("for x in xs.into_iter() {\nxs = vec![x];"), "{rs}");
    assert!(rs.contains("for x in (zs_id(&xs[..])).to_vec().into_iter() {"), "{rs}");
    // an element matched while its array is replaced in the arm: copied first (it cannot be moved out)
    assert!(rs.contains("match xs[(0i32) as usize].to_vec() {"), "{rs}");
    // `&&`'s right operand may not run: the push on its left copies
    assert!(rs.contains("({ let mut __zs_v = xs.to_vec(); __zs_v.push(2.0f64); __zs_v })"), "{rs}");
    // a local borrowed by one argument and consumed by the other is copied for the consumer only
    assert!(rs.contains("zs_pair(&xs[..], &(zs_keep(xs.to_vec()))[..])"), "{rs}");
    // iterated and reassigned in the loop: the loop iterates a copy, and the push inside still reuses the array
    assert!(rs.contains("for x in xs.to_vec().into_iter() {\nxs = { let mut __zs_v = xs;"), "{rs}");
}

/// examples/text.zsc: strings and `==` by content in @wasm. Lengths and order are JS's (UTF-16 code units: `"😀"` is
/// 2 long, and `"｡" > "😀"` though UTF-8 orders them the other way), and `==` on arrays, records and enums compares
/// contents in JS (`$eq`) and in Rust (derive(PartialEq))
#[test]
fn text_same_in_js_and_wasm() {
    let mut outs = vec![];
    let mut rs = String::new();
    for (name, flags) in [("text-wasm", &[][..]), ("text-js", &["--all-js"][..])] {
        let d = out_dir(name);
        let o = compare_build(&root().join("examples/text.zsc"), &d, flags);
        assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
        outs.push(node(&d, "const m = await import('./text.js'); console.log(JSON.stringify([m.text(4), m.shout('  hé '), m.conflict_report(5), m.card({ name: 'Ana`', age: 30 }, 21n), m.card({ name: '', age: -5 }, -4611686018427387904n), m.later_hole(), m.paren_text()]))"));
        if flags.is_empty() {
            rs = std::fs::read_to_string(d.join("text.wasm.rs")).unwrap();
        }
    }
    assert_eq!(outs[0], outs[1], "wasm and JS differ");
    assert_eq!(outs[0], "[\"Hello, bo! ANA,BO,😀X abc 3 110 2 TFTFTT 0;1;2;3;\",\"HÉ!\",\"FTxya,b5\",\"Ana` (30): 42 {not a hole} ${nor this} true\",\" (-5): -9223372036854775808 {not a hole} ${nor this} false\",\"<1>|a\\nb2\",2]\n");
    // a template string is appended piece by piece, its holes borrowed: no `format!` (core::fmt is about 10 KB of
    // wasm), and integers (in templates and `to_string`) are written by zs_push_int
    assert!(rs.contains("__zs_t.push_str(&p.name[..]); __zs_t.push_str(\" (\"); zs_push_int(&mut __zs_t, (p.age) as i64);"), "{rs}");
    assert!(!rs.contains("format!("), "{rs}");
    // borrowed strings, a borrowed result with a lifetime, `==` through borrows, and a string grown in place
    assert!(rs.contains("fn zs_longest<'a>(a: &'a str, b: &'a str) -> &'a str"), "{rs}");
    assert!(rs.contains("fn zs_same(p: &ZsPerson, q: &ZsPerson) -> bool {\n{\n(p == q)"), "{rs}");
    assert!(rs.contains("s = { let mut __zs_s = { let mut __zs_s = s;"), "{rs}");
}

/// tests/eq_chain.zsc: `==` on a type with exponentially many paths to the same types checks each type once
#[test]
fn eq_check_is_linear() {
    // a cycle on the type itself (eq_chain), and one that closes at the outermost type (eq_outer)
    for name in ["eq_chain", "eq_outer"] {
        let d = out_dir(name);
        let t = std::time::Instant::now();
        let o = zsc(&root().join(format!("tests/{name}.zsc")), &d, &[]);
        assert!(o.status.success(), "{name}: {}", String::from_utf8_lossy(&o.stderr));
        assert!(t.elapsed().as_secs() < 5, "{name} took {:?}", t.elapsed());
    }
}

/// tests/wire.zsc: records holding strings and arrays, arrays of records and strings, nested arrays cross to @wasm
/// as bytes (zustscript-helper on the JS side, zustscript_helper in the Rust), and give the JS layer's results
#[test]
fn wire_same_in_js_and_wasm() {
    let script = r#"
const m = await import('./wire.js');
const ana = { name: "Ana", age: 30, tags: ["a", "😀"] }, bo = { name: "bo", age: 41, tags: [] };
const t = { title: "T", people: [ana, bo], scores: [[1, 2.5], [], [3]], open: true, id: 9007199254740993n };
const g = m.grow(t, { name: "cy", age: 7, tags: ["x"] });
console.log(JSON.stringify([m.describe(ana), m.oldest([ana, bo, ana]), m.rename(bo, "Bö"), m.total(t), m.names(g), m.grid(3)]));
console.log(JSON.stringify(g, (k, v) => typeof v === "bigint" ? v.toString() : v));
// the helper keeps a leading U+FEFF and a field named __proto__
const h = await import('zustscript-helper');
const rec = ["rec", [["__proto__", "str"]]];
const notes = [{ text: "a", tag: { TAG: "Named", _0: "héllo" }, mood: "Happy" }, { text: "b", tag: { TAG: "Anon" }, mood: "Sad" }, { text: "c", tag: { TAG: "Pair", _0: 7, _1: [1, 2] }, mood: "Happy" }];
console.log(JSON.stringify([notes.map((n) => m.tag_name(n.tag)), notes.map((n) => m.retag(n)), m.tags(notes)]));
console.log(m.shadow(ana, 2), h.decode("str", h.encode("str", "\uFEFFhi")).length, JSON.stringify(h.decode(rec, h.encode(rec, JSON.parse('{"__proto__":"x"}')))));
"#;
    let want = "[\"Ana (30) a/😀\",{\"name\":\"bo\",\"age\":41,\"tags\":[]},{\"name\":\"Bö\",\"age\":41,\"tags\":[]},6.5,[\"ANA\",\"BO\",\"CY\"],[[0],[0,1],[0,1,2]]]\n{\"title\":\"T\",\"people\":[{\"name\":\"Ana\",\"age\":30,\"tags\":[\"a\",\"😀\"]},{\"name\":\"bo\",\"age\":41,\"tags\":[]},{\"name\":\"cy\",\"age\":7,\"tags\":[\"x\"]}],\"scores\":[[1,2.5],[],[3],[1.5]],\"open\":false,\"id\":\"9007199254740994\"}\n[[\"héllo\",\"-\",\"7/2\"],[{\"text\":\"a\",\"tag\":{\"TAG\":\"Pair\",\"_0\":5,\"_1\":[1.5]},\"mood\":\"Sad\"},{\"text\":\"b\",\"tag\":{\"TAG\":\"Named\",\"_0\":\"x\"},\"mood\":\"Happy\"},{\"text\":\"c\",\"tag\":{\"TAG\":\"Anon\"},\"mood\":\"Sad\"}],[{\"TAG\":\"Named\",\"_0\":\"héllo\"},{\"TAG\":\"Anon\"},{\"TAG\":\"Pair\",\"_0\":7,\"_1\":[1,2]}]]\n32 3 {\"__proto__\":\"x\"}\n";
    for flags in [&[][..], &["--all-js"][..]] {
        let d = out_dir(if flags.is_empty() { "wire" } else { "wire_js" });
        let o = compare_build(&root().join("tests/wire.zsc"), &d, flags);
        assert!(o.status.success(), "zsc {flags:?} failed:\n{}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(node(&d, script), want, "{flags:?}");
    }
    // (the wasm build goes through the helpers)
    let js = std::fs::read_to_string(root().join("target/test-out/wire/wire.js")).unwrap();
    assert!(js.contains("from \"zustscript-helper\""), "{js}");
}

/// tests/ts_import.zsc: records read from a TS file (tests/ts/model.ts) with `import type`. The JS takes TS's objects
/// as they are, and the .d.ts imports model.ts's types, so TS code passes its own values (tests/ts/consumer.ts,
/// checked with tsc when it is available)
#[test]
fn ts_records() {
    let d = out_dir("ts_import");
    // (the module and its TS file side by side, as in a project: the .d.ts imports `./ts/model.js`)
    std::fs::create_dir_all(d.join("ts")).unwrap();
    std::fs::copy(root().join("tests/ts/model.ts"), d.join("ts/model.ts")).unwrap();
    std::fs::copy(root().join("tests/ts/cr.ts"), d.join("ts/cr.ts")).unwrap();
    std::fs::copy(root().join("tests/ts_import.zsc"), d.join("ts_import.zsc")).unwrap();
    let o = zsc(&d.join("ts_import.zsc"), &d, &[]);
    assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
    let out = node(
        &d,
        r#"
const m = await import('./ts_import.js');
const u = { id: 7, name: "ana", admin: false, tags: ["a"] };
const p = m.reply({ author: u, title: "Hi", scores: [1, 2, 4.5], replies: [] }, u, "ok");
console.log(m.display(u), "|", m.summary(p), "|", JSON.stringify(m.promote(u)));
const doc = { title: "D", status: "draft", shapes: [{ kind: "circle", r: 1 }, { kind: "rect", w: 2, h: 3 }] };
console.log(m.label("published"), m.label(doc.status), m.total_area(doc), JSON.stringify(m.publish(doc)), m.same_shape({ kind: "rect", w: 2, h: 3 }, { kind: "rect", h: 3, w: 2 }), m.same_shape(doc.shapes[0], doc.shapes[1]), m.tile({ kind: "square", side: 4 }), m.tile({ kind: "empty" }));
console.log(m.area_w({ kind: "circle", r: 1 }), m.area_w({ kind: "rect", w: 2, h: 3 }), m.label_w("published"), JSON.stringify(m.publish_w(doc)) === JSON.stringify(m.publish(doc)));
console.log(m.same_flag({ kind: "on" }, { kind: "on" }), m.same_flag({ kind: "on" }, { kind: "off" }), JSON.stringify(m.item_of({ item: { kind: "crate", n: 3 } })), m.crates({ item: { kind: "nothing" } }), m.esc("done"), m.par("p"), m.loc("l2"), m.sur("bc"), m.sur_first() === "\uD835\uDC9Ca");
console.log(m.same_user(u, { ...u, tags: ["a"] }), m.same_user(u, m.promote(u)), m.dist({ x: 3, y: 4 }, { x: 0, y: 0 }), m.badge(m.promote(u)), m.cr({ a: 2, b: "x", c: true }));
"#,
    );
    assert_eq!(out, "ana (member, #7) | Hi by ana (member, #7): 1 replies, score 7 | {\"id\":7,\"name\":\"ana\",\"admin\":true,\"tags\":[\"a\",\"staff\"]}\nlive draft! 9 {\"title\":\"D\",\"status\":\"published\",\"shapes\":[{\"kind\":\"circle\",\"r\":2},{\"kind\":\"rect\",\"w\":3,\"h\":2}]} true false 4 0\n3 6 live true\ntrue false {\"kind\":\"crate\",\"n\":3} 0 d q 2 2 true\ntrue false 5 ANA #7 a+staff x2!\n");
    let dts = std::fs::read_to_string(d.join("ts_import.d.ts")).unwrap();
    assert!(dts.contains("import type { Doc, Esc, Flag, Loc, Par, Point, Post, Shape, Shelf, Status, Sur, Tile, User } from \"./ts/model.js\";"), "{dts}");
    assert!(!dts.contains("export interface User"), "{dts}");
    // (an unexported union with a named member: written with the member's object, which needs no name)
    assert!(dts.contains("export type Item = { kind: \"crate\"; n: number } | { kind: \"nothing\" };"), "{dts}");
    let tsc = std::env::var("TSC").unwrap_or_else(|_| "tsc".into());
    if Command::new(&tsc).arg("--version").output().is_err() {
        eprintln!("skipped tsc: no tsc (set TSC)");
        return;
    }
    std::fs::copy(root().join("tests/ts/consumer.ts"), d.join("consumer.ts")).unwrap();
    std::fs::write(d.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
    let o = Command::new(&tsc)
        .current_dir(&d)
        .args(["--strict", "--noEmit", "--target", "es2022", "--module", "nodenext", "--moduleResolution", "nodenext", "consumer.ts"])
        .output()
        .unwrap();
    assert!(o.status.success(), "tsc:\n{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
}

/// tests/modules: app.zsc imports types, functions, an `@wasm fn` and a component from shapes.zsc. Each module is
/// compiled on its own into one directory; the JS imports `./shapes.js`, the .d.ts imports its types, and TS code
/// passes a type of one module to the other (tests/modules/consumer.ts, checked with tsc when it is available)
#[test]
fn modules_import_types_and_functions() {
    let d = out_dir("modules");
    let m = root().join("tests/modules");
    for f in ["shapes.zsc", "app.zsc"] {
        let o = zsc(&m.join(f), &d, &[]);
        assert!(o.status.success(), "{f}: {}", String::from_utf8_lossy(&o.stderr));
    }
    let js = std::fs::read_to_string(d.join("app.js")).unwrap();
    assert!(js.contains("import { Badge, area, first, twice } from \"./shapes.js\";"), "{js}");
    let dts = std::fs::read_to_string(d.join("app.d.ts")).unwrap();
    assert!(dts.contains("import type { Labeled, Shape } from \"./shapes.js\";"), "{dts}");
    assert!(!dts.contains("interface Labeled"), "{dts}");
    let out = node(&d, "const m = await import('./app.js'); const { render } = await import('react/jsx-runtime'); console.log(m.demo(), render(m.view({ label: 'x', shape: { TAG: 'Circle', _0: { x: 0, y: 0 }, _1: 1 } })));");
    assert_eq!(out, "9 3 none 42 c <span>x</span>\n");

    // what cannot be imported, and a cycle
    let errs = d.join("errs.zsc");
    for (src, want) in [
        ("import { helper } from \"./shapes.zsc\";", "does not export `helper` (write `export fn helper` there)"),
        ("import { nothing } from \"./shapes.zsc\";", "declares no type or function `nothing`"),
        ("import type { area } from \"./shapes.zsc\";", "`area` is a function: `import { area } from"),
        ("import { area } from \"./shapes.ts\";", "a TS file gives types only"),
        ("import { a } from \"./cycle_a.zsc\";", "an import cycle"),
        (include_str!("modules/wasm_calls_import.zsc"), "calling `area` (imported from another module, so it is JS here)"),
    ] {
        std::fs::write(&errs, src).unwrap();
        for f in ["shapes.zsc", "cycle_a.zsc", "cycle_b.zsc"] {
            std::fs::copy(m.join(f), d.join(f)).unwrap();
        }
        let o = zsc(&errs, &d.join("errs-out"), &[]);
        let e = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success() && e.contains(want), "{src}: {e}");
    }

    let tsc = std::env::var("TSC").unwrap_or_else(|_| "tsc".into());
    if Command::new(&tsc).arg("--version").output().is_err() {
        eprintln!("skipped tsc: no tsc (set TSC)");
        return;
    }
    std::fs::copy(m.join("consumer.ts"), d.join("consumer.ts")).unwrap();
    std::fs::write(d.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
    let o = Command::new(&tsc)
        .current_dir(&d)
        .args(["--strict", "--noEmit", "--skipLibCheck", "--target", "es2022", "--module", "nodenext", "--moduleResolution", "nodenext", "consumer.ts"])
        .output()
        .unwrap();
    assert!(o.status.success(), "tsc:\n{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
}

/// tests/modules: a type two modules import comes in once (by the file that defines it), a type of a module in
/// another directory is imported from that module's JS as seen from the importer, a lowercase type comes in with the
/// function that uses it, an imported component takes a generic record of props, and a component whose JS has
/// another name is written by that name
#[test]
fn modules_diamond_and_directories() {
    let d = out_dir("modules_diamond");
    let m = root().join("tests/modules");
    for (f, o) in [("common.zsc", "."), ("left.zsc", "."), ("lib/leaf.zsc", "lib"), ("lib/mid.zsc", "lib"), ("diamond.zsc", ".")] {
        let o = zsc(&m.join(f), &d.join(o), &[]);
        assert!(o.status.success(), "{f}: {}", String::from_utf8_lossy(&o.stderr));
    }
    let dts = std::fs::read_to_string(d.join("diamond.d.ts")).unwrap();
    assert!(dts.contains("import type { Leaf } from \"./lib/leaf.js\";"), "{dts}");
    let out = node(&d, "const m = await import('./diamond.js'); const { render } = await import('react/jsx-runtime'); console.log(m.both(), JSON.stringify(m.leaf(7)), render(m.view()));");
    assert_eq!(out, "6 {\"v\":7} <b>hi</b>\n");

    let src = d.join("named.zsc");
    std::fs::write(&src, "type P = { label: String }\nextern \"react\" fn Fancy(props: P) -> Jsx = \"UI.Badge\";\nexport fn v() -> Jsx { <Fancy label=\"x\" /> }\n").unwrap();
    let o = zsc(&src, &d.join("named"), &[]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let js = std::fs::read_to_string(d.join("named/named.js")).unwrap();
    assert!(js.contains("import { UI } from \"react\";") && js.contains("_jsx(UI.Badge, { label: \"x\" })"), "{js}");
}

/// tests/js_inplace.zsc: the JS layer appends in place only onto a local array no one else can see, so every
/// result is what the copying push gives
#[test]
fn js_push_in_place() {
    let d = out_dir("js_inplace");
    let o = zsc(&root().join("tests/js_inplace.zsc"), &d, &[]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = node(&d, "const m = await import('./js_inplace.js'); const xs = [1]; const r = m.from_param(xs); console.log(JSON.stringify([m.ramp(5), m.aliased(), [r, xs.length], m.iterate_and_grow(), m.stored(), m.other_local(), m.concat_and_short(), m.compare_with_push(), m.push_arg_assigns(), m.jsx_alias(), m.computed_callee(), m.jsx_key().key, m.handler_twice(), m.handler_alternatives()]))");
    assert_eq!(out, "[5,12,[2,1],4,114,13,31,false,2,1,1,1,4,1]\n");
    // == by content compares the declared fields only, so it is symmetric: a value from TS may carry more
    let out = node(&d, "const m = await import('./js_inplace.js'); const a = { x: 1, name: 'a' }, b = { x: 1, name: 'a', extra: 2 }; console.log(JSON.stringify([m.same_p(a, b), m.same_p(b, a), m.same_p(a, { x: 2, name: 'a' })]))");
    assert_eq!(out, "[true,true,false]\n");
    let js = std::fs::read_to_string(d.join("js_inplace.js")).unwrap();
    // in place exactly where no one else can see the array: the loop's accumulator, a local made by a copying
    // push, and a concat onto a fresh literal
    assert_eq!(js.matches(".push(").count(), 2, "{js}");
    assert!(js.contains("acc = (acc.push(i), acc);") && js.contains("b = (b.push(3), b);") && js.contains("a = $append(a, [2, 3]);"), "{js}");
}

/// examples/kernels.zsc: the `@simd` kernels (v128, a module zsc writes), the same functions through zustc
/// (`--no-simd`) and as JS (`--all-js`) agree: exactly for `map` and i32 sums, to 12 digits for f64 sums (the lanes
/// regroup them)
#[test]
fn kernels_simd_nosimd_js_agree() {
    let src = root().join("examples/kernels.zsc");
    let mut outs = vec![];
    for (name, flags) in [("kernels-simd", &[][..]), ("kernels-nosimd", &["--no-simd"][..]), ("kernels-js", &["--all-js"][..])] {
        let d = out_dir(name);
        let o = zsc(&src, &d, flags);
        assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(d.join("kernels.simd.wasm").exists(), flags.is_empty());
        let r = Command::new("node").arg(root().join("tests/kernels_driver.mjs")).arg(d.join("kernels.js")).output().unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        outs.push(String::from_utf8(r.stdout).unwrap());
    }
    assert_eq!(outs[0], outs[2], "@simd and JS differ");
    assert_eq!(outs[1], outs[2], "--no-simd and JS differ");
    assert!(outs[0].contains("axpy_exact true"));
    assert!(outs[0].contains("isum [-290105161,-2147483639]"));
}

/// The guideline: i64 arithmetic in the JS layer is a warning (BigInt); in a @wasm fn it is not
#[test]
fn i64_in_js_warns() {
    let o = zsc(&root().join("examples/basics.zsc"), &out_dir("warn-basics"), &[]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("warning: i64 arithmetic in the JS layer runs on BigInt"));
    let o = zsc(&root().join("examples/numeric.zsc"), &out_dir("warn-numeric"), &[]);
    assert!(!String::from_utf8_lossy(&o.stderr).contains("warning"), "{}", String::from_utf8_lossy(&o.stderr));
}

/// examples/ml.zsc: f32 kernels on F32Array (f32x4), @simd(relaxed) (fused multiply-add, relaxed max) and a
/// matrix-vector product with row offsets agree across @simd, --no-simd and --all-js (sums to 5 digits)
#[test]
fn ml_kernels_agree() {
    let src = root().join("examples/ml.zsc");
    let mut outs = vec![];
    for (name, flags) in [("ml-simd", &[][..]), ("ml-nosimd", &["--no-simd"][..]), ("ml-js", &["--all-js"][..])] {
        let d = out_dir(name);
        let o = zsc(&src, &d, flags);
        assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
        let r = Command::new("node").arg(root().join("tests/ml_driver.mjs")).arg(d.join("ml.js")).output().unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        outs.push(String::from_utf8(r.stdout).unwrap());
    }
    assert_eq!(outs[0], outs[2], "@simd and JS differ");
    assert_eq!(outs[1], outs[2], "--no-simd and JS differ");
    assert!(outs[0].contains("dense_is_f32array true"));
    assert!(outs[0].contains("relu [0,0.5,0,2,0,7,0.125]"));
    assert!(outs[0].contains("bounds_error true"));
}

/// The review of mizchi/zust#31: match bindings that shadow the subject, shadowing and nested blocks, a lone
/// match arm, a range's end read once, precedence after a call, a callback evaluated once, a generated temporary
/// named like a parameter (@wasm), ReactElement imported for a record's Jsx field
#[test]
fn regressions() {
    let d = build("regress");
    let out = node(
        &d,
        r#"
const m = await import('./regress.js');
console.log(m.unwrap_or({ TAG: "Some", _0: 4 }, 0), m.unwrap_or({ TAG: "None" }, 9));
console.log(m.shadow(), m.only("One"), m.range_end_once(), m.prec(-1, 2, 3));
console.log(m.add_all([1, 2]).join(","), m.push_v([1, 2], 3).join(","));
console.log(m.shadow_fn(), m.prec_str(3), m.push_zs([1], 2).join(","), m.re({ value: 5 }));
console.log(m.zs_inj(5, 2), m.kw_inj(7, 3), JSON.stringify(m.kw_rec({ async_: 1, move: 2 })));
console.log(m.many_ops(100000), m.own_ops(), m.nested_k(), m.proto_op());
console.log(m.gen_after_k(100000), m.gen_outer_op(100000), m.gen_plus_one(100000));
try { m.resume_twice(); } catch (e) { console.log(e.message); }
const g = m.leaks_far(), y = g.next();
console.log(JSON.stringify(y.value), JSON.stringify(g.next(41)), m.handles_far(), m.paren_str());
"#,
    );
    assert_eq!(out, "4 9\n12 1 3 9\n2,3 1,2,3\n7 6 1,2 5\n3 4 {\"async_\":2,\"move\":1}\n99999 42 101 7\n99999 199999 100000\nzustscript: a continuation was resumed twice\n{\"TAG\":\"FarOp\"} {\"value\":42,\"done\":true} 42 ()!\n");
    let dts = std::fs::read_to_string(d.join("regress.d.ts")).unwrap();
    assert!(dts.contains("import type { ReactElement as $ReactElement } from \"react\";"), "{dts}");
    assert!(dts.contains("export interface ReactElement {"), "{dts}");
}

/// A `.d.ts` without a Jsx type imports no ReactElement, even when a name in it contains `$ReactElement`
/// (a type parameter named ReactElement makes the specialization `f$i32$ReactElement`)
#[test]
fn no_jsx_no_react_import() {
    let d = out_dir("no_jsx");
    let src = d.join("nojsx.zsc");
    std::fs::write(&src, "export fn f<T: Num, ReactElement>(x: T) -> T { x }\nexport fn g(x: i32) -> i32 { f(x) }\n").unwrap();
    let o = zsc(&src, &d, &[]);
    assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
    let dts = std::fs::read_to_string(d.join("nojsx.d.ts")).unwrap();
    assert!(!dts.contains("import type"), "{dts}");
}

/// `--check-externs`: each extern's TS declaration is checked against its zustscript signature with tsrs (the
/// `TSRS` variable, or `tsrs` on PATH; skipped otherwise). app.zsc's `useState` fits the react stand-in's
/// declaration; tests/externs/bad.zsc gives the setter the wrong type, and the error points at that extern.
#[test]
fn externs_match_ts() {
    let tsrs = std::env::var("TSRS").unwrap_or_else(|_| "tsrs".into());
    if Command::new(&tsrs).arg("--version").output().is_err() {
        eprintln!("skipped: no tsrs (set TSRS)");
        return;
    }
    let d = out_dir("externs");
    let o = zsc(&root().join("examples/app.zsc"), &d, &["--check-externs"]);
    assert!(o.status.success(), "zsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("externs match their TS declarations (1 checked with tsrs)"));
    let o = zsc(&root().join("tests/externs/bad.zsc"), &d, &["--check-externs"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{err}");
    assert!(err.contains("the TS declaration of `useState` does not fit this signature"), "{err}");
    assert!(err.contains("bad.zsc:2:1"), "{err}");
    assert!(!err.contains("`log`"), "console.log fits: {err}");
    // a record type (the TS side returns plain numbers) and a dotted extern (`api.read`) both check
    std::fs::write(d.join("lib.d.ts"), "export declare function point(): { x: number; y: number };\nexport declare const api: { read(): string };\n").unwrap();
    std::fs::write(d.join("lib.js"), "export const point = () => ({ x: 1, y: 2 });\nexport const api = { read: () => \"r\" };\n").unwrap();
    let o = zsc(&root().join("tests/externs/named.zsc"), &d, &["--check-externs"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success() && err.contains("(2 checked with tsrs)"), "{err}");
    // a record's Jsx field, reached only by expanding the record: ReactElement is still imported
    std::fs::write(d.join("lib.d.ts"), "import type { ReactElement } from \"react\";\nexport declare function view(): { title: string; body: ReactElement };\n").unwrap();
    let o = zsc(&root().join("tests/externs/jsx.zsc"), &d, &["--check-externs"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success() && err.contains("(1 checked with tsrs)"), "{err}");
}

#[test]
fn runtime_name_collisions() {
    // the shared runtime and a module named `zsc-runtime` never overwrite each other in one directory
    let d = out_dir("runtime_names");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let src = d.join("zsc-runtime.zsc");
    std::fs::write(&src, "export fn one() -> i32 { 1 }\n").unwrap();
    let o = zsc(&src, &d, &[]);
    assert!(!o.status.success(), "a module named like the runtime is refused in shared mode");
    assert!(zsc(&root().join("examples/basics.zsc"), &d, &[]).status.success());
    let rt = std::fs::read_to_string(d.join("zsc-runtime.js")).unwrap();
    let o = zsc(&src, &d, &["--inline-runtime"]);
    assert!(!o.status.success(), "an inline `zsc-runtime` must not overwrite the shared runtime");
    assert_eq!(std::fs::read_to_string(d.join("zsc-runtime.js")).unwrap(), rt);
    // in its own directory, the inline build works; a shared build into that directory is then refused
    let e = d.join("inline");
    assert!(zsc(&src, &e, &["--inline-runtime"]).status.success());
    let o = zsc(&root().join("examples/basics.zsc"), &e, &[]);
    assert!(!o.status.success(), "the shared runtime must not overwrite a module named like it");
    assert!(std::fs::read_to_string(e.join("zsc-runtime.js")).unwrap().contains("function one"));
}

#[test]
fn rust_source_for_plain_functions() {
    let d = out_dir("plain-rust");
    let src = d.join("math.zsc");
    std::fs::write(&src, "type Reading = { value: i32 }\nfn twice(n: i32) -> i32 { n + n }\nexport fn total(xs: Array<Reading>) -> i32 { xs.fold(0, (s, x) => s + twice(x.value)) }").unwrap();
    let out = zsc(&src, &d, &["--emit-rust", "--zustc", "unused-zustc"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!d.join("math.js").exists() && !d.join("math.wasm").exists());
    let driver = d.join("driver.rs");
    std::fs::write(&driver, "mod math; fn main() { assert_eq!(math::zs_total(&[math::ZsReading { value: 21 }]), 42); }").unwrap();
    let binary = d.join("run");
    let build = Command::new("rustc").arg("--edition=2024").arg(driver).arg("-o").arg(&binary).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    assert!(Command::new(binary).status().unwrap().success());
}

#[test]
fn wasm_can_call_a_rust_compatible_plain_helper() {
    let d = out_dir("plain-helper");
    let src = d.join("mixed.zsc");
    std::fs::write(&src, "fn helper(n: i32) -> i32 { n + 1 }\n@wasm export fn calc(n: i32) -> i32 { helper(n) }\nexport fn twice(n: i32) -> i32 { calc(n) * 2 }").unwrap();
    let out = zsc(&src, &d, &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(node(&d, "const m = await import('./mixed.js'); console.log(m.twice(20));"), "42\n");
}

#[test]
fn wasm_requires_rust_even_for_unused_functions() {
    let d = out_dir("rust-required");
    let src = d.join("require.zsc");
    std::fs::write(&src, "@wasm fn only_rust() -> i32 { 1 }").unwrap();
    let js = zsc(&src, &d, &["--all-js"]);
    assert!(!js.status.success());
    assert!(String::from_utf8_lossy(&js.stderr).contains("requires the Rust backend"));
    assert!(!d.join("require.js").exists());
    std::fs::write(&src, "extern fn host(x: i32) -> i32 = \"host\";\nfn h(x: i32) -> i32 { host(x) }\n@wasm fn unused(x: i32) -> i32 { h(x) }").unwrap();
    for flags in [&[][..], &["--emit-rust"][..]] {
        let bad = zsc(&src, &d, flags);
        assert!(!bad.status.success());
        assert!(String::from_utf8_lossy(&bad.stderr).contains("calling `host`"));
    }
}
