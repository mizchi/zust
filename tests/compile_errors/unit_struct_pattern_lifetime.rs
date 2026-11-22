struct G<const N: usize>;
fn main() {
    let g = G::<5>;
    let G::<'static, 5> = g;
}
