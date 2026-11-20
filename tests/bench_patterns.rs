//! The programs of bench/gen_patterns.py (the shapes of application code the benchmarks measure) build with zustc and
//! print what rustc's build prints. Every pattern at 2 groups, and `mix` with a changed constant (the edit the KPIs
//! measure). Without python3 or rustc on PATH, the test says so and passes.
#![cfg(feature = "acg")]

use std::process::Command;

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// A new directory only this user can enter, made where nothing was: a directory that is there already (another
/// user's, made before the test started) is never reused, so nothing the test writes goes through it. It is removed
/// when the guard is dropped, a failing assertion's unwinding too
struct PrivateDir(std::path::PathBuf);

impl std::ops::Deref for PrivateDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for PrivateDir {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn private_dir(prefix: &str) -> PrivateDir {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::temp_dir();
    for k in 0..100u32 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = base.join(format!("{prefix}_{}_{t}_{k}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return PrivateDir(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("{}: {e}", dir.display()),
        }
    }
    panic!("no new directory under {}", base.display());
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
    let dir = private_dir("bench_patterns");
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
        let r = Command::new(&reference).output().unwrap();
        assert!(r.status.success(), "rustc's build of {name} exited with {:?}: {}", r.status.code(), String::from_utf8_lossy(&r.stderr));
        let want = r.stdout;
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
