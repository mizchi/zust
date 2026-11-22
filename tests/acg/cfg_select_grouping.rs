// `cfg_select!` in an expression keeps a `{ .. }` arm's grouping; `std::cfg_select!`; an arm in a match
fn main() {
    let a = 2 * cfg_select! { _ => { 1 + 2 } };
    let b = std::cfg_select! { unix => { 1 } _ => { 2 } };
    let c = match a { 6 => cfg_select! { _ => { a + b } }, _ => 0 };
    cfg_select! {
        unix => {
            let d = a + c;
            println!("{} {} {} {}", a, b, c, d);
        }
    }
}
