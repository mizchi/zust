//! The programs of bench/gen_patterns.py (the shapes of application code the benchmarks measure) build with zustc and
//! print what rustc's build prints. Every pattern at 2 groups, and `mix` with a changed constant (the edit the KPIs
//! measure). Without python3 or rustc on PATH, the test says so and passes.
#![cfg(feature = "acg")]

use std::process::Command;

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn bench_patterns_like_rustc() {
    if !have("python3") || !have("rustc") {
        eprintln!("skipped: python3 and rustc are needed");
        return;
    }
    let list = Command::new("python3").args(["bench/gen_patterns.py", "--list"]).output().unwrap();
    let mut cases: Vec<(String, &str, &str)> = String::from_utf8(list.stdout).unwrap().lines().map(|p| (p.to_string(), "2", "0")).collect();
    assert!(cases.len() >= 9, "{cases:?}");
    cases.push(("mix".into(), "2", "3"));
    let dir = std::env::temp_dir().join(format!("bench_patterns_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut bad = vec![];
    for (name, n, variant) in &cases {
        let g = Command::new("python3").args(["bench/gen_patterns.py", name, n, variant]).output().unwrap();
        assert!(g.status.success(), "gen_patterns.py {name}");
        let src = String::from_utf8(g.stdout).unwrap();
        let path = dir.join(format!("{name}.rs"));
        std::fs::write(&path, &src).unwrap();
        let reference = dir.join(format!("{name}.rustc"));
        let r = Command::new("rustc").args(["--edition", "2021", "-Awarnings"]).arg(&path).arg("-o").arg(&reference).output().unwrap();
        assert!(r.status.success(), "rustc {name}: {}", String::from_utf8_lossy(&r.stderr));
        let want = Command::new(&reference).output().unwrap().stdout;
        let exe = dir.join(name);
        if let Err(e) = zustc::driver2::build(&src, &exe, false, None) {
            bad.push(format!("{name}: build: {e}"));
            continue;
        }
        let o = Command::new(&exe).output().unwrap();
        if !o.status.success() || o.stdout != want {
            bad.push(format!(
                "{name}: exit {:?}\n--- got\n{}--- want\n{}",
                o.status.code(),
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&want)
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
