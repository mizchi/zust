fn main() {
    println!("{}", app::banner());
    #[cfg(feature = "with-extra")]
    println!("extra: {}", extra::answer());
    #[cfg(not(feature = "with-extra"))]
    println!("no extra");
}
