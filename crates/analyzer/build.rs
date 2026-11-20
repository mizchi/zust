// The executables are linked non-PIE on x86-64 Linux: a PIE spends ~10% of a no-op build (~0.17M instructions) on the
// dynamic loader's relocations at every start. Only this package's binaries: proc-macro crates must stay shared
// objects
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target == "x86_64-unknown-linux-gnu" {
        println!("cargo:rustc-link-arg-bins=-no-pie");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
