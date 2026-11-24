// A call's turbofish gives the fn's type arguments to the inference: `let c = 66; id::<u32>(c)` makes c a u32 (not
// an i32 to be mismatched), also through a fn item with every type argument written (`let f = id::<u64>; f(c)`)
fn id<T>(t: T) -> T { t }
fn pair<A, B>(a: A, b: B) -> (A, B) { (a, b) }
fn main() {
    let c = 66;
    id::<u32>(c);
    let d = 7;
    let r = id::<&u8>(&d);
    let e = 300;
    let f = id::<u64>;
    let g = f(e);
    let (x, y) = pair::<i8, u16>(1, 2);
    println!("{} {} {} {} {}", c, r, g, x, y);
    println!("{}", std::mem::size_of_val(&c) + std::mem::size_of_val(&e));
}
