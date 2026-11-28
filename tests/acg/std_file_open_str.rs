// File::open / fs::metadata with a path given as a &str literal, a String and a &String (before, only a Path::new(..)
// argument compiled: the NUL check `p.iter().any(..)` needed the slice iterator, which nothing selected), and a path
// with a NUL in it (std's InvalidInput)
use std::fs::{self, File};

fn main() {
    let a = File::open("/dev/null").is_ok();
    let owned = String::from("/dev/null");
    let b = File::open(&owned).is_ok();
    let c = File::open(owned).is_ok();
    println!("{} {} {}", a, b, c);
    println!("{}", fs::metadata("/dev/null").map(|m| m.is_file()).unwrap_or(true));
    match File::open("bad\0path") {
        Ok(_) => println!("opened"),
        Err(e) => println!("{:?} {}", e.raw_os_error(), e),
    }
}
