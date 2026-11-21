struct Y;
impl Y {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
fn main() {
    let _ = Y == Y;
}
