fn identity<T>(x: T) -> T { x }
fn main() {
    let _ = identity::<true>([()]);
}
