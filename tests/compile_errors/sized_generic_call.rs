fn takes<T>(_: &T) {}

fn main() {
    let s: &str = "abc";
    takes(s);
}
