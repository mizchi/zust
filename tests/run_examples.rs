//! Builds and runs tests/probe, examples and tests/acg with session (minclif), and checks whether the output (stdout, stderr, exit code) is
//! the same as tests/expected/<directory>.<name>.{stdout,stderr,code}. The expected values were made with the first syn + Cranelift driver
//! (that driver was removed). The Cranelift backend is compared with minclif by tests/cranelift_backend.rs.
//! DRIVER2_FILE=path for just 1 file.
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;

#[path = "common/par.rs"]
mod par;

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
fn outputs_match_expected() {
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
    let results = par::par_map(&files, |p| -> Result<(), String> {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let d = p.parent().unwrap().file_name().unwrap().to_string_lossy().to_string();
        let exp = |ext: &str| PathBuf::from("tests/expected").join(format!("{d}.{name}.{ext}"));
        let (Ok(want_out), Ok(want_err), Ok(want_code)) = (std::fs::read(exp("stdout")), std::fs::read(exp("stderr")), std::fs::read_to_string(exp("code"))) else {
            return Err(format!("{d}/{name}: no expected output in tests/expected"));
        };
        let src = std::fs::read_to_string(p).unwrap();
        // (a directory of its own for each source directory: two of them have a file of the same name)
        let exe = dir.join(&d).join(&name);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        if let Err(e) = zustc::session::build(&src, &exe, false, None) {
            return Err(format!("{name}: {}", e.lines().next().unwrap_or("")));
        }
        let a = std::process::Command::new(&exe).current_dir("tests/expected").stdin(std::process::Stdio::null()).output().unwrap();
        let code = a.status.code().unwrap_or_else(|| 128 + a.status.signal().unwrap_or(0));
        if a.stdout == want_out && a.stderr == want_err && code.to_string() == want_code.trim() {
            Ok(())
        } else {
            let why = if a.stdout != want_out { "stdout" } else if a.stderr != want_err { "stderr" } else { "exit code" };
            Err(format!("{d}/{name}: {why} differs (exit {code} vs {})", want_code.trim()))
        }
    });
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let bad: Vec<String> = results.into_iter().filter_map(|r| r.err()).collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{ok} same\n{}", bad.join("\n"));
}

/// tests/editions/NAME.rs, built at each edition E that tests/expected/editions.NAME.E.stdout is given for, prints that
/// (rustc's output at that edition)
#[test]
fn edition_dependent_programs_print_rustc_s_output_at_each_edition() {
    let dir = private_dir("zust_editions");
    let mut bad = vec![];
    let mut n = 0;
    for ent in std::fs::read_dir("tests/editions").unwrap() {
        let p = ent.unwrap().path();
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&p).unwrap();
        for edition in [2015u16, 2018, 2021, 2024] {
            let Ok(want) = std::fs::read(format!("tests/expected/editions.{name}.{edition}.stdout")) else { continue };
            n += 1;
            let exe = dir.join(format!("{name}_{edition}"));
            let mut s = zustc::session::Session::new(zustc::session::Options { no_cache: true, jobs: 1, edition, ..Default::default() });
            if let Err(e) = s.build(&src, &exe) {
                bad.push(format!("{name} ({edition}): {}", e.lines().next().unwrap_or("")));
                continue;
            }
            let a = std::process::Command::new(&exe).output().unwrap();
            if a.stdout != want || !a.status.success() {
                bad.push(format!("{name} ({edition}): stdout differs (exit {:?})", a.status.code()));
            }
        }
    }
    assert!(n >= 2 && bad.is_empty(), "{n} runs\n{}", bad.join("\n"));
}
