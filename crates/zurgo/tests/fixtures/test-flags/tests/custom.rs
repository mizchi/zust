// (`harness = false`: its own main, one test that passes when it exits with 0)
fn main() {
    assert_eq!(tf::one(), 1);
    println!("custom ran");
}
