// (`harness = false`: its own main, one test that passes when it exits with 0. Built with `cfg(test)`, as cargo builds
// it: this `main` is there only then)
#[cfg(test)]
fn main() {
    assert_eq!(tf::one(), 1);
    println!("custom ran");
}
