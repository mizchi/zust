// (the package's library: the binary below uses it by the package's name)
pub fn banner() -> String {
    format!("app {}", greet::greeting("zurgo"))
}
