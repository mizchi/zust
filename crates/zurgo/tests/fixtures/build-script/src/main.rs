include!(concat!(env!("OUT_DIR"), "/x.rs"));

fn main() {
    println!("answer {} / {}", env!("ANSWER"), from_build());
    println!("gen {} {} {}", gen::value(), gen::flavor(), gen::compiled_with());
    println!("cfg has_answer {} level0 {}", cfg!(has_answer), cfg!(level = "0"));
    println!("pkg {} {} {}.{}.{} pre={}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), env!("CARGO_PKG_VERSION_MAJOR"), env!("CARGO_PKG_VERSION_MINOR"), env!("CARGO_PKG_VERSION_PATCH"), env!("CARGO_PKG_VERSION_PRE"));
    println!("crate {} bin {} desc {:?}", env!("CARGO_CRATE_NAME"), env!("CARGO_BIN_NAME"), option_env!("CARGO_PKG_DESCRIPTION"));
    println!("profile {} feats [{}] cfg {}", env!("PROFILE_SEEN"), env!("FEATS"), env!("CFG_ARCH"));
}
