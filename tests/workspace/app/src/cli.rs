// (a dependency under another name: `u::pick` is `util::pick`)
use util as u;

pub fn run(n: i64) {
    let v = [1, 2, 3];
    if n == 2 {
        println!("{}", v[n as usize + 5]);
    }
    if n == 3 {
        let x: Option<u8> = None;
        println!("{}", x.unwrap());
    }
    println!("pick {}", u::pick(&[4, 5], 1));
    println!("div {}", geom::div(10, n));
}
