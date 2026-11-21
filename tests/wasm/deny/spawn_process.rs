//! Spawning a process needs fork / exec, which the host does not give: the module is not run at all.
fn main() {
    let out = std::process::Command::new("sh").arg("-c").arg("echo escaped").output();
    println!("{}", out.is_ok());
}
