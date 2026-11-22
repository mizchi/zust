fn main() {
    println!("cargo:answer=42");
    println!("cargo::metadata=flavor=umami");
    if std::env::var("CARGO_FEATURE_SPICY").is_ok() {
        println!("cargo:rustc-cfg=spicy");
    }
    println!("cargo:rustc-env=GEN_FLAVOR=umami");
    // (compiled with the package's features, as cargo compiles build.rs)
    // (and the features cargo lists in CARGO_CFG_FEATURE)
    let feats = std::env::var("CARGO_CFG_FEATURE").unwrap_or_else(|_| "unset".to_string());
    #[cfg(feature = "spicy")]
    println!("cargo:rustc-env=GEN_COMPILED_WITH=spicy/{}", feats);
    #[cfg(not(feature = "spicy"))]
    println!("cargo:rustc-env=GEN_COMPILED_WITH=none/{}", feats);
}
