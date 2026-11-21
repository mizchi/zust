//! Writing a host file: denied, and nothing appears on the host (the test checks the path).
fn main() {
    let r = std::fs::write("zust_wasm_sandbox_probe.txt", "escaped");
    println!("{}", if r.is_err() { "denied" } else { "written" });
}
