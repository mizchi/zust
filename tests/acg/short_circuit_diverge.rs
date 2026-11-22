//! A chain of `&&` / `||` whose middle operand diverges: the operands before it may still short-circuit to the end,
//! whose value is the chain's
fn f(a: bool) -> bool {
    a && { return true; } && false
}

fn g(a: bool) -> bool {
    a || { return false; } || true
}

fn main() {
    println!("{} {} {} {}", f(false), f(true), g(true), g(false));
}
