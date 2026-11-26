// `stringify!(m!())` keeps the call as text, as rustc does (#199: both macro expanders expanded m in it), with each
// delimiter, through `std::stringify!`, and beside a call of m that does expand
macro_rules! m {
    () => {
        1 + 2
    };
}
fn main() {
    println!("{} {}", stringify!(m!()), m!());
    println!("{}", std::stringify!(m!() + m!()));
    println!("{}", stringify!(x = m![]; y = m! {}));
    println!("{}", stringify! { m!() });
}
