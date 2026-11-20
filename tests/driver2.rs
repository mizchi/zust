//! tests/probe・examples・tests/acg を driver2 (minclif) でビルドして動かし、出力 (stdout・stderr・終了コード) が
//! tests/expected/<ディレクトリ>.<名前>.{stdout,stderr,code} と同じか。期待値は最初の syn + Cranelift のドライバで作ったもの
//! (そのドライバは消した)。Cranelift のバックエンドは tests/driver2_cranelift.rs が minclif と比べる。
//! DRIVER2_FILE=path で 1 ファイルだけ。
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;

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
fn driver2_matches_expected() {
    let dir = private_dir("drv2");
    let mut files: Vec<PathBuf> = vec![];
    for d in ["tests/probe", "examples", "tests/acg"] {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    files.sort();
    if let Ok(f) = std::env::var("DRIVER2_FILE") {
        files = vec![PathBuf::from(f)];
    }
    let (mut ok, mut bad) = (0, vec![]);
    for p in &files {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let d = p.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        let exp = |ext: &str| PathBuf::from("tests/expected").join(format!("{d}.{name}.{ext}"));
        let (Ok(want_out), Ok(want_err), Ok(want_code)) = (std::fs::read(exp("stdout")), std::fs::read(exp("stderr")), std::fs::read_to_string(exp("code"))) else {
            bad.push(format!("{d}/{name}: no expected output in tests/expected"));
            continue;
        };
        let src = std::fs::read_to_string(p).unwrap();
        let exe = dir.join(&name);
        if let Err(e) = zustc::driver2::build(&src, &exe, false, None) {
            bad.push(format!("{name}: {}", e.lines().next().unwrap_or("")));
            continue;
        }
        let a = std::process::Command::new(&exe).current_dir("tests/expected").stdin(std::process::Stdio::null()).output().unwrap();
        let code = a.status.code().unwrap_or_else(|| 128 + a.status.signal().unwrap_or(0));
        if a.stdout == want_out && a.stderr == want_err && code.to_string() == want_code.trim() {
            ok += 1;
        } else {
            let why = if a.stdout != want_out { "stdout" } else if a.stderr != want_err { "stderr" } else { "exit code" };
            bad.push(format!("{d}/{name}: {why} differs (exit {code} vs {})", want_code.trim()));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{ok} same\n{}", bad.join("\n"));
}
