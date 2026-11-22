// `match g { G::<5> => .. }`: a unit struct's turbofish pattern in a match, through a reference too
struct G<const N: usize>;
fn main() {
    let g = G::<5>;
    match g {
        G::<5> => println!("five"),
    }
    match &g {
        G::<5> => println!("ref"),
    }
}
