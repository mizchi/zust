//! Native SIMD (issue #30): tests/simd/*.rs use core::arch::x86_64's intrinsics (SSE .. SSE4.2, AVX, AVX2, FMA:
//! 128- and 256-bit vectors). zustc's builds (the Cranelift backend, which holds a 256-bit vector as two 128-bit
//! halves) print what rustc's builds print, with and without -O. minclif has no SIMD and says so.
//! Skipped on a CPU without AVX2 and FMA (the programs' kernels need them to run under rustc).
#![cfg(feature = "acg")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zust_simd_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn programs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir("tests/simd").unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "rs")).collect();
    v.sort();
    v
}

fn cpu_ok() -> bool {
    let ok = std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") && std::is_x86_feature_detected!("sse4.2");
    if !ok {
        eprintln!("simd: this CPU has no AVX2 / FMA, skipped");
    }
    ok
}

fn zustc(src: &Path, out: &Path, flags: &[&str]) -> Result<(), String> {
    let o = Command::new(env!("CARGO_BIN_EXE_zustc")).arg("--no-cache").args(flags).arg(src).arg("-o").arg(out).output().unwrap();
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

#[test]
fn x86_simd_like_rustc() {
    if !cpu_ok() {
        return;
    }
    let d = dir("x86");
    let progs = programs();
    assert!(progs.len() >= 3);
    let mut bad = vec![];
    for p in &progs {
        let n = p.file_stem().unwrap().to_string_lossy().into_owned();
        let rbin = d.join(format!("{n}.rustc"));
        let r = Command::new("rustc").args(["-Awarnings", "--edition", "2021"]).arg(p).arg("-o").arg(&rbin).output().unwrap();
        assert!(r.status.success(), "{n}: rustc: {}", String::from_utf8_lossy(&r.stderr));
        let want = Command::new(&rbin).output().unwrap();
        for flags in [&[][..], &["-O"][..]] {
            let exe = d.join(format!("{n}{}", flags.join("")));
            if let Err(e) = zustc(p, &exe, flags) {
                bad.push(format!("{n} {flags:?}: zustc: {e}"));
                continue;
            }
            let got = Command::new(&exe).output().unwrap();
            if got.status.code() != want.status.code() || got.stdout != want.stdout {
                let (w, g) = (String::from_utf8_lossy(&want.stdout).into_owned(), String::from_utf8_lossy(&got.stdout).into_owned());
                let first = w.lines().zip(g.lines()).find(|(a, b)| a != b).map(|(a, b)| format!("rustc: {a}\n  zustc: {b}")).unwrap_or_default();
                bad.push(format!("{n} {flags:?}: exit {:?} / {:?}, first difference:\n  {first}", want.status.code(), got.status.code()));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&d);
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// minclif has no SIMD yet: it rejects a program with vectors (it does not miscompile it)
#[test]
fn minclif_rejects_simd() {
    let d = dir("minclif");
    for p in ["tests/simd/avx.rs", "tests/wasm/simd/kernels.rs"] {
        let e = zustc(Path::new(p), &d.join("m"), &["--backend=minclif"]).unwrap_err();
        assert!(e.contains("minclif has no SIMD yet"), "{p}: {e}");
    }
    let _ = std::fs::remove_dir_all(&d);
}
