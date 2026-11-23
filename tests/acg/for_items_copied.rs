// The items of a loop through `copied` / `cloned` are copies: they hold what v holds, not the loan of v. A loop over
// a value inside a loop over `&v` has items of its own
fn main() {
    let n = 1;
    let mut v = vec![&n];
    let mut saved = &n;
    for x in v.iter().copied() {
        saved = x;
    }
    v.clear();
    println!("{}", saved);
    let mut w = vec![String::from("a")];
    let mut keep = String::new();
    for x in w.iter().cloned() {
        keep = x;
    }
    w.clear();
    println!("{}", keep);
    let mut u = vec![1, 2];
    let mut last = &n;
    for _x in &u {
        for y in [&n] {
            last = y;
        }
    }
    u.clear();
    println!("{} {}", last, u.len());
}
