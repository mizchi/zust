//! Whether the locations in panic messages (`panicked at <file>:<line>:<col>:`) are the same as rustc's.
//! Builds tests/panic_loc/cases.rs with rustc and with zustc (the session, minclif) and compares the cases for arguments 0 to 6
//! (out-of-range indexing (Vec / array), overflow, unwrap, assert!, division by 0, expect inside a format macro's arguments).
#![cfg(feature = "acg")]

fn location(stderr: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(stderr);
    let line = s.lines().find(|l| l.contains("panicked at "))?;
    // Only the line and column of `<file>:<line>:<col>:`
    let rest = line.split("panicked at ").nth(1)?.trim_end_matches(':');
    let mut parts = rest.rsplitn(3, ':');
    let col = parts.next()?;
    let ln = parts.next()?;
    Some(format!("{ln}:{col}"))
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
fn panic_locations_like_rustc() {
    let dir = private_dir("panic_loc");
    let src_path = "tests/panic_loc/cases.rs";
    let src = std::fs::read_to_string(src_path).unwrap();
    let r = dir.join("r");
    let st = std::process::Command::new("rustc").args(["-Awarnings", "--edition", "2024", src_path, "-o"]).arg(&r).status().unwrap();
    assert!(st.success());
    let d = dir.join("d");
    zustc::session::build(&src, &d, false, None).unwrap();
    let mut bad = vec![];
    for k in 0..7 {
        let a = std::process::Command::new(&r).arg(k.to_string()).output().unwrap();
        let b = std::process::Command::new(&d).arg(k.to_string()).output().unwrap();
        let (la, lb) = (location(&a.stderr), location(&b.stderr));
        if la.is_none() || la != lb || a.status.code() != b.status.code() {
            bad.push(format!("case {k}: rustc {la:?} (exit {:?}) vs session {lb:?} (exit {:?})", a.status.code(), b.status.code()));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
