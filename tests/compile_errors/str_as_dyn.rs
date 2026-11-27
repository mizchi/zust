// (a str has no size for a vtable: `"test" as &dyn Any` is E0277, as cast/trait-object-size-error-14366.rs)
fn main() {
    let _x = "test" as &dyn std::any::Any;
}
