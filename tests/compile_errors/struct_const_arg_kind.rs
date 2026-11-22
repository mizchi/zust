struct Flag<const B: bool>;
fn main() {
    let _ok = Flag::<true>;
    let _bad = Flag::<1>;
}
