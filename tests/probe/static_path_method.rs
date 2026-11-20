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
    println!("{} {} {}", config::NAMES.len(), config::NAMES[1].len(), config::TITLE.len());
    println!("{}", config::SIZES[2].count_ones());
    println!("{:?}", service::widths());
}
