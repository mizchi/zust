//! `zustc --test`: a crate's `#[test]` functions with the harness of zust_syntax::test_harness (a test per process,
//! libtest's command line), against what rustc's `--test` build of the same file does. And `--cfg`.

use std::path::{Path, PathBuf};
use std::process::Command;

const ZUSTC: &str = env!("CARGO_BIN_EXE_zustc");

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn out(exe: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let o = Command::new(exe).args(args).output().unwrap();
    (o.status.code(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

/// The tests rustc's harness lists, each with what it reports: "ok", "FAILED" or "ignored"
fn outcomes(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix("test ")?.split_once(" ... ").map(|(n, r)| (n.trim_end_matches(" - should panic").to_string(), r.to_string())))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_iter()
        .collect()
}

#[test]
fn harness_like_rustc() {
    let d = dir("test_harness");
    let src = Path::new("tests/test_harness/lib.rs");
    let z = d.join("z");
    let o = Command::new(ZUSTC).args(["--test", "--no-cache"]).arg(src).arg("-o").arg(&z).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let r = d.join("r");
    let o = Command::new("rustc").args(["--test", "--edition", "2021", "-Awarnings"]).arg(src).arg("-o").arg(&r).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // (the list, in libtest's terse form: with and without --ignored)
    for a in [&["--list", "--format", "terse"][..], &["--list", "--format", "terse", "--ignored"][..]] {
        assert_eq!(out(&z, a).1, out(&r, a).1, "{a:?}");
    }
    // (the whole run, each test in a child: the same outcomes and the same exit code)
    let (zc, zo, _) = out(&z, &[]);
    let (rc, ro, _) = out(&r, &[]);
    assert_eq!(outcomes(&zo), outcomes(&ro), "\n{zo}\n---\n{ro}");
    assert_eq!(zc, rc);
    let tail = |s: &str| s.lines().rfind(|l| l.starts_with("test result:")).map(|l| l.split("; finished").next().unwrap().to_string());
    assert_eq!(tail(&zo), tail(&ro));
    // (one test, as nextest and zurgo run them: the exit code says it)
    for (name, extra) in [("tests::adds", &[][..]), ("tests::fails", &[]), ("tests::panics", &[]), ("tests::panics_wrong", &[]), ("tests::result", &[]), ("tests::slow", &["--ignored"][..])] {
        let mut a = vec![name, "--exact", "--nocapture"];
        a.extend(extra);
        assert_eq!(out(&z, &a).0 == Some(0), out(&r, &a).0 == Some(0), "{name}");
    }
    // (`--ignored` with `--exact`: a test that is not ignored does not run)
    assert_eq!(out(&z, &["tests::fails", "--exact", "--ignored"]).0, out(&r, &["tests::fails", "--exact", "--ignored"]).0);
    // (with a filter: substrings)
    assert_eq!(outcomes(&out(&z, &["panic"]).1), outcomes(&out(&r, &["panic"]).1));
    let _ = std::fs::remove_dir_all(&d);
}

/// A `#[test]` that a `macro_rules!` makes runs too: the tests are collected from the expanded source
#[test]
fn macro_made_tests_run() {
    let d = dir("test_harness_macro");
    let src = d.join("m.rs");
    std::fs::write(&src, "macro_rules! make {\n    () => {\n        #[test]\n        fn generated() {}\n    };\n}\nmake!();\nmacro_rules! named {\n    ($attr:ident) => {\n        #[$attr]\n        fn generated2() { panic!(\"ran\") }\n    };\n}\nnamed!(test);\n#[test]\nfn plain() {}\n").unwrap();
    let o = Command::new(ZUSTC).args(["--test", "--no-cache"]).arg(&src).arg("-o").arg(d.join("m")).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(!err.contains("not run yet"), "{err}");
    assert_eq!(out(&d.join("m"), &["--list", "--format", "terse"]).1, "generated: test\ngenerated2: test\nplain: test\n");
    assert_eq!(out(&d.join("m"), &["generated2", "--exact"]).0, Some(101));
    let _ = std::fs::remove_dir_all(&d);
}

/// A `#[should_panic]` test passes on a panic only: an exit with 101 and no panic fails it. (Not in the comparison
/// with rustc: there, `exit` ends every test of the process)
#[test]
fn should_panic_needs_a_panic() {
    let d = dir("test_harness_exit");
    let src = d.join("e.rs");
    std::fs::write(&src, "#[test]\n#[should_panic]\nfn exits() {\n    std::process::exit(101);\n}\n#[test]\n#[should_panic]\nfn panics_empty() {\n    panic!(\"\");\n}\n").unwrap();
    let o = Command::new(ZUSTC).args(["--test", "--no-cache"]).arg(&src).arg("-o").arg(d.join("e")).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_ne!(out(&d.join("e"), &["exits", "--exact"]).0, Some(0));
    assert_eq!(out(&d.join("e"), &["panics_empty", "--exact"]).0, Some(0));
    let _ = std::fs::remove_dir_all(&d);
}

/// Under `--test` the crate's `main` is an ordinary function, whatever its signature: a test may call one with
/// parameters. Built on its own, the same `main` is an error
#[test]
fn main_with_parameters() {
    let d = dir("test_harness_main_params");
    let src = d.join("p.rs");
    std::fs::write(&src, "fn main(value: i32) -> i32 {\n    value\n}\n#[test]\nfn calls() {\n    assert_eq!(main(3), 3);\n}\n").unwrap();
    let o = Command::new(ZUSTC).args(["--test", "--no-cache"]).arg(&src).arg("-o").arg(d.join("p")).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(out(&d.join("p"), &["calls", "--exact"]).0, Some(0));
    let o = Command::new(ZUSTC).args(["--no-cache"]).arg(&src).arg("-o").arg(d.join("q")).output().unwrap();
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("`main` must be `fn main()`"));
    let _ = std::fs::remove_dir_all(&d);
}

/// `--cfg name` and `--cfg name="value"`, as rustc takes them
#[test]
fn cfg_flag() {
    let d = dir("cfg_flag");
    let src = d.join("c.rs");
    std::fs::write(&src, "fn main() {\n    #[cfg(foo)]\n    println!(\"foo\");\n    #[cfg(level = \"2\")]\n    println!(\"level 2\");\n    #[cfg(not(test))]\n    println!(\"not test\");\n}\n").unwrap();
    let e = d.join("c");
    let o = Command::new(ZUSTC).args(["--no-cache", "--cfg", "foo", "--cfg=level=\"2\""]).arg(&src).arg("-o").arg(&e).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(out(&e, &[]).1, "foo\nlevel 2\nnot test\n");
    let _ = std::fs::remove_dir_all(&d);
}
