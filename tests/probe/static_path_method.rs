#![allow(non_snake_case)]
// A method on a static reached through a module path: `m::S.len()`, `m::S[i].len()`, `super::m::S[i]` (the type was
// not known before the call, so the method was looked up on the wrong type)
mod config {
    pub static NAMES: [&str; 3] = ["alpha", "beta", "gamma"];
    pub static TITLE: &str = "report";
    pub static SIZES: [u32; 3] = [7, 8, 255];
}

mod service {
    pub fn widths() -> Vec<usize> {
        (0..3).map(|i| super::config::NAMES[i].len()).collect()
    }
}

fn main() {
    // (a local of the same name is not the static: `config::SIZES` is the static's, `SIZES` the local's)
    let SIZES: [u16; 2] = [300, 400];
    println!("{} {} {}", config::SIZES[0], config::SIZES.len(), SIZES[1]);
    let got: u32 = crate::config::SIZES[0];
    let got2: usize = self::config::SIZES.len();
    let TITLE: &str = "local";
    println!("{} {} {} {}", got, got2, crate::config::TITLE, TITLE);
    println!("{} {} {}", config::NAMES.len(), config::NAMES[1].len(), config::TITLE.len());
    println!("{}", config::SIZES[2].count_ones());
    println!("{:?}", service::widths());
}
