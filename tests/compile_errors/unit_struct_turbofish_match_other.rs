struct G<const N: usize>;
fn main() {
    let g = G::<5>;
    match g {
        G::<6> => {}
    }
}
