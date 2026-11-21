//! The prototype's tests: examples/*.rys compile and run under node with the expected output, the output passes
//! `node --check`, and tests/errors/*.rys fail with the message their first line expects.
//! (app.rys needs zustc: built in the zust checkout this crate sits in, `cargo build --release` there)

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rsc(input: &Path, out: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rsc")).arg(input).arg("-o").arg(out).output().unwrap()
}

fn out_dir(name: &str) -> PathBuf {
    let d = root().join("target/test-out").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("node_modules")).unwrap();
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
    let o = rsc(&root().join(format!("examples/{name}.rys")), &d);
    assert!(o.status.success(), "rsc failed:\n{}", String::from_utf8_lossy(&o.stderr));
    let c = Command::new("node").arg("--check").arg(d.join(format!("{name}.js"))).output().unwrap();
    assert!(c.status.success(), "the output is not valid JS:\n{}", String::from_utf8_lossy(&c.stderr));
    d
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
    let js = std::fs::read_to_string(d.join("basics.js")).unwrap();
    assert!(!js.contains("class "), "the output uses no classes");
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
        let o = rsc(&f, &root().join("target/test-out/errors"));
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success(), "{} compiled", f.display());
        assert!(err.contains(want), "{}: expected `{want}` in:\n{err}", f.display());
        n += 1;
    }
    assert!(n >= 10);
}
