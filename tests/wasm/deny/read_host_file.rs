//! Reading a host file: open fails inside the sandbox (only the virtual /proc/self/cmdline exists).
fn main() {
    match std::fs::read_to_string("/etc/passwd") {
        Ok(s) => println!("read {} bytes", s.len()),
        Err(_) => println!("denied"),
    }
    match std::fs::read_to_string("Cargo.toml") {
        Ok(s) => println!("read {} bytes", s.len()),
        Err(_) => println!("denied"),
    }
}
