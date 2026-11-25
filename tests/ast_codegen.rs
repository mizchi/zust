//! Builds tests/acg/*.rs with the path that uses neither syn nor Cranelift (driver2 + acg + minclif) and compares the output with the rustc build.
#![cfg(feature = "acg")]

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

#[path = "common/par.rs"]
mod par;

#[test]
fn acg_like_rustc() {
    let dir = private_dir("acg");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir("tests/acg").unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    // (ACG_FILE=path tries just one file)
    if let Ok(f) = std::env::var("ACG_FILE") {
        files = vec![std::path::PathBuf::from(f)];
    }
    let bad: Vec<String> = par::par_map(&files, |p| -> Result<(), String> {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(p).unwrap();
        let obj = match zustc::driver2::build_object(&src, false) {
            Ok(o) => o,
            Err(e) => return Err(format!("{name}: {e}")),
        };
        let o = dir.join(format!("{name}.o"));
        std::fs::write(&o, &obj).unwrap();
        let exe = dir.join(&name);
        let st = std::process::Command::new("cc").arg(&o).arg("-o").arg(&exe).status().unwrap();
        if !st.success() {
            return Err(format!("{name}: link failed"));
        }
        let ours = std::process::Command::new(&exe).output().unwrap();
        let rexe = dir.join(format!("{name}.rustc"));
        let st = std::process::Command::new("rustc")
            .args(["-Awarnings", "--edition", "2024", "-C", "panic=abort", "-C", "link-arg=-lc", "-C", "link-arg=-Wl,--defsym=rust_eh_personality=0"])
            .arg(p)
            .arg("-o")
            .arg(&rexe)
            .status()
            .unwrap();
        assert!(st.success());
        let theirs = std::process::Command::new(&rexe).output().unwrap();
        if ours.stdout != theirs.stdout || ours.status.code() != theirs.status.code() {
            return Err(format!(
                "{name}: output differs:\n{}\nvs rustc:\n{}",
                String::from_utf8_lossy(&ours.stdout),
                String::from_utf8_lossy(&theirs.stdout)
            ));
        }
        Ok(())
    })
    .into_iter()
    .filter_map(|r| r.err())
    .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
