use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let answer = env::var("DEP_GEN_ANSWER").unwrap_or_else(|_| "none".to_string());
    let code = format!("pub fn from_build() -> String {{ String::from(\"{}\") }}\n", helper::shout(&answer));
    fs::write(out.join("x.rs"), code).unwrap();
    println!("cargo:rustc-cfg=has_answer");
    println!("cargo::rustc-cfg=level=\"{}\"", env::var("OPT_LEVEL").unwrap());
    println!("cargo:rustc-env=ANSWER={answer}");
    println!("cargo:rustc-env=PROFILE_SEEN={}", env::var("PROFILE").unwrap());
    let feats: Vec<String> = env::vars().map(|(k, _)| k).filter(|k| k.starts_with("CARGO_FEATURE_")).collect();
    println!("cargo:rustc-env=FEATS={}", feats.join(","));
    println!("cargo:rustc-env=CFG_ARCH={} {} {}", env::var("CARGO_CFG_TARGET_ARCH").unwrap(), env::var("CARGO_CFG_TARGET_OS").unwrap(), env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
}
