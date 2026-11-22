struct NN;
struct RR;
impl PartialEq<RR> for [NN; 1] {
    fn eq(&self, _: &RR) -> bool {
        true
    }
}
fn main() {
    let _ = [NN] == RR;
    let _ = [NN] == [NN];
}
