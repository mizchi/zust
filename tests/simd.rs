//! Native SIMD (issue #30): tests/simd/*.rs use core::arch::x86_64's intrinsics (SSE .. SSE4.2, AVX, AVX2, FMA:
//! 128- and 256-bit vectors). zustc's builds (the Cranelift backend, which holds a 256-bit vector as two 128-bit
//! halves) print what rustc's builds print, with and without -O. minclif has no SIMD and says so.
//! Skipped on a CPU without AVX2 and FMA (the programs' kernels need them to run under rustc).
#![cfg(feature = "acg")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A new directory of this test's own (created, not found: mode 0700, a fresh name if one exists)
fn dir(name: &str) -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    for k in 0..100u32 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let d = std::env::temp_dir().join(format!("zust_simd_{name}_{}_{t}_{k}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&d) {
            Ok(()) => return d,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("{}: {e}", d.display()),
        }
    }
    panic!("no new directory under {}", std::env::temp_dir().display());
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
            if !want.status.success() || !got.status.success() {
                bad.push(format!(
                    "{n} {flags:?}: exit {:?} / {:?}\n  rustc's stderr: {}\n  zustc's stderr: {}",
                    want.status.code(),
                    got.status.code(),
                    String::from_utf8_lossy(&want.stderr).trim(),
                    String::from_utf8_lossy(&got.stderr).trim()
                ));
            } else if got.stdout != want.stdout {
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

/// Calls rustc rejects are rejected: a store through a `*const` pointer, a lane index past u32 (not truncated to
/// another lane), a half of a 128-bit vector, a lane out of range
#[test]
fn simd_errors() {
    let d = dir("errors");
    let cases: &[(&str, &str)] = &[
        ("let v = _mm_set1_ps(1.0); let x = [0f32; 4]; unsafe { _mm_storeu_ps(x.as_ptr(), v) };", "takes `*mut f32`"),
        ("let v = _mm_set1_ps(1.0); let x = [0i32; 4]; unsafe { _mm_storeu_ps(x.as_mut_ptr(), v) };", "takes `*mut f32`"),
        ("let a = _mm_set1_epi32(1); let _ = _mm_extract_epi32::<4294967297>(a);", "too large"),
        ("let a = _mm_set1_epi32(1); let _ = _mm_extract_epi32::<4>(a);", "out of range"),
        ("let a = _mm_set1_ps(1.0); let _ = _mm_extractf128_ps::<0>(a);", "_mm_extractf128_ps"),
        ("let a = _mm_set1_epi32(1); let _ = _mm_extract_epi32::<BIG>(a); } const BIG: usize = 4294967297; fn f() {", "too large"),
        ("let a = _mm_set1_ps(1.0); let _ = _mm_dp_ps::<256>(a, a);", "out of range"),
        ("let a = _mm_set1_ps(1.0); let _ = _mm_cmp_ps::<32>(a, a);", "out of range"),
        ("let t = [0i32; 8]; let i = _mm_set1_epi32(0); let _ = unsafe { _mm_i32gather_epi32::<3>(t.as_ptr(), i) };", "scale"),
        ("let _ = core::arch::wasm32::_mm_set1_ps(1.0);", "_mm_set1_ps"),
        ("let _ = core::x86_64::arch::_mm_set1_ps(1.0);", "_mm_set1_ps"),
        ("let a = _mm_set1_pd(1.0); let _ = _mm_blend_pd::<4>(a, a);", "out of range"),
        ("let a = _mm_set1_ps(1.0); let _ = _mm_blend_ps::<16>(a, a);", "out of range"),
        ("let a = _mm_set1_epi32(1); let _ = _mm_extract_epi32::<[u8; 1]>(a);", "not a constant"),
        ("let a = _mm_set1_epi32(1); let _ = _mm_extract_epi32::<A>(a); } type A = [u8; 1]; fn f() {", "not a constant"),
        ("let a = _mm_set1_epi32(1); let _ = g::<[u8; 1]>(a); } fn g<T>(a: __m128i) -> i32 { _mm_extract_epi32::<T>(a) } fn f() {", "not a constant"),
        ("let a = _mm_set1_epi32(1); let _ = g::<[(); 1]>(a); } fn g<T>(a: __m128i) -> i32 { _mm_extract_epi32::<T>(a) } fn f() {", "not a constant"),
        ("let a = _mm_set1_epi32(1); let _ = g::<[(); 1]>(a); } fn g<T>(a: __m128i) -> i32 { _mm_extract_epi32::<{ T }>(a) } fn f() {", "not a constant"),
        ("let mut x = [0f32; 4]; let v = _mm_set1_ps(1.0); unsafe { _mm_maskstore_ss(x.as_mut_ptr(), v, v) };", "`_mm_maskstore_ss` is not supported yet"),
    ];
    let mut bad = vec![];
    for (k, (body, want)) in cases.iter().enumerate() {
        let src = d.join(format!("e{k}.rs"));
        std::fs::write(&src, format!("use std::arch::x86_64::*;\nfn main() {{ {body} }}\n")).unwrap();
        match zustc(&src, &d.join(format!("e{k}")), &[]) {
            Ok(()) => bad.push(format!("{body}: built")),
            Err(e) if !e.contains(want) => bad.push(format!("{body}: {e}")),
            Err(_) => {}
        }
    }
    let _ = std::fs::remove_dir_all(&d);
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
