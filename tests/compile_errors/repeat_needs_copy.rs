struct Token(String);
fn main() {
    let names: [Option<Token>; 3] = [None; 3];
    let _ = names;
}
