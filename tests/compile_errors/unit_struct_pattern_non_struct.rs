struct G<const N: usize>;
fn main() {
    let _ = matches!(0u8, G::<_>);
}
