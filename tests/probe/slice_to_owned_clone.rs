// A slice's to_owned (its to_vec) of a derive(Clone) type with a String in it, the program's only clone (#280's
// review: the prelude's `__exr_clone` was not selected with it)
#[derive(Clone, Debug)]
struct P { name: String, n: u8 }
fn main() {
    let v = vec![P { name: "a".to_string(), n: 1 }, P { name: "b".to_string(), n: 2 }];
    let s: &[P] = &v;
    let o = s.to_owned();
    println!("{:?}", o);
    let t = v[..1].to_owned();
    println!("{:?}", t);
}
