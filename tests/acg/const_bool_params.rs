// `const B: bool` / `const N: u8`: const parameters of other types than usize, `Flag<true>` / `f::<true>()`
struct Flag<const B: bool>;
impl<const B: bool> Flag<B> {
    fn get(&self) -> bool {
        B
    }
    fn label(&self) -> &'static str {
        if B { "on" } else { "off" }
    }
}
impl Flag<true> {
    fn only_true(&self) -> u8 {
        1
    }
}
impl Flag<false> {
    fn only_false(&self) -> u8 {
        0
    }
}
fn pick<const B: bool>() -> u32 {
    match B {
        true => 10,
        false => 20,
    }
}
struct Small<const N: u8>;
impl<const N: u8> Small<N> {
    fn next(&self) -> u8 {
        N + 1
    }
}
fn show(f: Flag<false>) -> bool {
    f.get()
}
fn main() {
    let t: Flag<true> = Flag;
    let f: Flag<false> = Flag;
    println!("{} {} {} {}", t.get(), t.only_true(), show(Flag), f.only_false());
    println!("{} {}", t.label(), f.label());
    println!("{} {}", pick::<true>(), pick::<false>());
    let s: Small<254> = Small;
    println!("{}", s.next());
    let u: Flag<{ 1 > 0 }> = Flag;
    println!("{}", u.get());
}
