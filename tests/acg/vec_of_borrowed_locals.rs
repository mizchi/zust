// A local borrowed in a macro's arguments (`vec![&mut x]`, `vec![&y]`) is in memory, as one borrowed elsewhere: it was
// "internal: borrowed local is not in memory"
fn main() {
    let mut x = 3;
    {
        let v = vec![&mut x];
        *v.into_iter().next().unwrap() += 1;
    }
    let mut y = 5;
    let w = vec![&y];
    println!("{} {}", x, w[0]);
    y += 1;
    println!("{}", y);
}
