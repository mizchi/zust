// A str trait's method called on a String goes through the deref, `s.as_str().custom_len()` (#256, #280's review:
// the prelude's `as_str` was not selected for it, in a program that formats nothing). The result is the exit code
trait Ext {
    fn custom_len(&self) -> usize;
}

impl Ext for str {
    fn custom_len(&self) -> usize {
        self.len() * 2
    }
}

fn main() {
    let n = String::from("xy").custom_len();
    std::process::exit(n as i32);
}
