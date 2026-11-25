//! A parallel build (forked children, or worker threads: ZUSTC_PAR=threads, #165) writes the same .o as `-j 1`.
//!
//! The case that once differed: a fn compiled in a child that coerces a closure with no captures to a fn pointer
//! (examples/coercions.rs). Its Blob named the closure's fn-pointer instance (request_fnptr), which the encoding writes
//! as the closure's own: the parent's code called the closure through a fn pointer with the environment's parameter,
//! and the program panicked ("attempt to multiply with overflow") where `-j 1`'s printed its results. A Blob that names
//! such an instance is now compiled again by the parent (sendable), as check_many does.

/// examples/coercions.rs with enough fns before it that the first wave (main among them) is split across 4 workers at
/// the default threshold (16 per worker)
fn program() -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/coercions.rs")).unwrap();
    let mut pads = String::new();
    let mut calls = String::from("    let mut acc = 0u64;\n");
    for k in 0..96 {
        pads.push_str(&format!("fn pad{k}(x: u64) -> u64 {{ x.wrapping_mul({}) ^ {k} }}\n", k + 3));
        calls.push_str(&format!("    acc = acc.wrapping_add(pad{k}(acc));\n"));
    }
    calls.push_str("    if acc == 1 { println!(\"{}\", acc); }\n");
    let i = src.find("fn main() {").unwrap();
    let j = i + src[i..].find('\n').unwrap() + 1;
    format!("{pads}{}{calls}{}", &src[..j], &src[j..])
}

#[test]
fn parallel_builds_write_the_object_of_one_job() {
    let dir = std::env::temp_dir().join(format!("zust_build_parallel_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("p.rs");
    std::fs::write(&src, program()).unwrap();
    // (the object and what the program prints, built with this mode and number of jobs; `split`: every wave split)
    let build = |par: &str, jobs: &str, split: bool| -> (Vec<u8>, String) {
        let out = dir.join(format!("p_{par}_{jobs}_{split}"));
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_zustc"));
        c.args(["--no-cache", "-j", jobs]).arg(&src).arg("-o").arg(&out).env("ZUSTC_PAR", par).env_remove("ZUSTC_BUILD_PAR_MIN");
        if split {
            c.env("ZUSTC_BUILD_PAR_MIN", "1");
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{par} -j{jobs}: {}", String::from_utf8_lossy(&o.stderr));
        let obj = std::fs::read(out.with_extension("o")).unwrap();
        let run = std::process::Command::new(&out).output().unwrap();
        (obj, format!("{:?} {}", run.status.code(), String::from_utf8_lossy(&run.stdout)))
    };
    let one = build("off", "1", false);
    assert!(one.1.starts_with("Some(0) 20 4 25 12 3"), "{}", one.1);
    for (par, split) in [("fork", false), ("threads", false), ("fork", true), ("threads", true)] {
        let many = build(par, "4", split);
        assert_eq!(many.1, one.1, "{par} -j4 (split {split}): what the program prints");
        assert!(many.0 == one.0, "{par} -j4 (split {split}): the .o differs from -j1's");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
