// A raw identifier in a macro's text keeps its `r#` (`stringify!(r#foo)` is "r#foo"), while it names `foo`
macro_rules! show {
    ($e:expr) => {
        println!("{} = {}", stringify!($e), $e)
    };
}
fn main() {
    let r#foo = 3;
    show!(r#foo + 1);
    println!("{}", stringify!(r#foo));
    println!("{}", stringify!(r#struct));
    println!("{}", stringify!(r#foo.r#bar(r#baz)));
    println!("{}", foo);
}
