//! Every safe access past the end of a buffer is a panic (exit 101), never a read or write out of
//! bounds. One argument per case.
//@ expect: panic
//@ args: 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20
fn arg() -> usize {
    std::env::args().nth(1).unwrap().parse().unwrap()
}
fn main() {
    let k = arg();
    let n = 3 + (k & 0); // 3, unknown at compile time
    let mut v = vec![1u32, 2, 3];
    let a = [7u8; 3];
    let s = String::from("añb"); // 'ñ' is 2 bytes: byte 2 is not a char boundary
    match k {
        0 => println!("{}", v[n]),
        1 => println!("{}", a[n]),
        2 => println!("{:?}", &v[1..n + 1]),
        3 => println!("{:?}", &v[n - 1..n - 2]),
        4 => println!("{}", &s[0..n - 1]),
        5 => println!("{:?}", v.split_at(n + 1).0),
        6 => println!("{:?}", v.get(n).unwrap()),
        7 => v.insert(n + 1, 9),
        8 => println!("{}", v.remove(n)),
        9 => println!("{}", v.swap_remove(n)),
        10 => println!("{}", v.drain(0..n + 1).count()),
        11 => println!("{}", v.split_off(n + 1).len()),
        12 => println!("{}", v.chunks(n - 3).count()),
        13 => println!("{}", v.windows(n - 3).count()),
        14 => {
            let mut t = s.clone();
            t.insert(n - 1, 'x');
            println!("{t}");
        }
        15 => {
            let mut t = s.clone();
            t.truncate(n - 1);
            println!("{t}");
        }
        16 => println!("{}", (0..10).step_by(n - 3).count()),
        17 => {
            let w: &mut [u32] = &mut v;
            w.swap(0, n);
        }
        18 => {
            let mut t = s.clone();
            println!("{}", t.remove(n - 1));
        }
        19 => {
            let mut t = s.clone();
            t.replace_range(0..n - 1, "z");
            println!("{t}");
        }
        _ => println!("{}", s.split_at(n - 1).0),
    }
    println!("not reached");
}
