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
    // (with a filter: substrings)
    assert_eq!(outcomes(&out(&z, &["panic"]).1), outcomes(&out(&r, &["panic"]).1));
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
