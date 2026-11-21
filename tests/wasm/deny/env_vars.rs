//! The host's environment is not visible: no variable is set inside the sandbox.
fn main() {
    println!("{:?} {:?}", std::env::var("HOME").ok(), std::env::var("PATH").ok());
}
