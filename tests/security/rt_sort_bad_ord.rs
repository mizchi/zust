//! Sorting with a comparison that is not a total order must stay memory safe: the result is a
//! permutation of the input (or a panic), never a duplicated or lost element (older std sorts
//! had panic-safety bugs here).
//@ pattern: trusting a user impl (an Ord that is not a total order: the class of CVE-2015-20001)
//@ expect: safe
fn main() {
    let mut v: Vec<String> = (0..50).map(|i| format!("{}", (i * 37) % 50)).collect();
    let mut want = v.clone();
    want.sort();
    let mut n = 0u32;
    v.sort_by(|a, b| {
        n = n.wrapping_mul(1103515245).wrapping_add(12345);
        if (n >> 16) % 3 == 0 { a.cmp(b) } else { b.len().cmp(&a.len()) }
    });
    let mut got = v.clone();
    got.sort();
    if got != want {
        println!("not a permutation");
        std::process::exit(1);
    }
    println!("ok permutation");
}
