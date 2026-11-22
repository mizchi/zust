struct G<const N: usize, const M: usize>;
fn main() {
    let g = G::<5, 7>;
    match g {
        G::<6, _> => {}
    }
}
