// `G::<5, _>` patterns: the `_` from the value's type
struct G<const N: usize, const M: usize>;
fn main() {
    let g = G::<5, 7>;
    match g {
        G::<5, _> => println!("ok"),
    }
    let G::<5, _> = &g;
}
