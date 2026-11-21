pub fn greeting(who: &str) -> String {
    let word = if cfg!(feature = "hello") { "hello" } else { "hi" };
    let s = format!("{word}, {who}");
    if cfg!(feature = "loud") { s.to_uppercase() } else { s }
}
