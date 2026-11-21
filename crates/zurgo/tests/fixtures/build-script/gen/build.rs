fn main() {
    println!("cargo:answer=42");
    println!("cargo::metadata=flavor=umami");
    if std::env::var("CARGO_FEATURE_SPICY").is_ok() {
        println!("cargo:rustc-cfg=spicy");
    }
    println!("cargo:rustc-env=GEN_FLAVOR=umami");
}
