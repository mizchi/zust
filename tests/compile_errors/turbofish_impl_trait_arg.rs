// A turbofish names a fn's explicit parameters only, not an `impl Trait` argument's (E0107)
fn foo<T: ?Sized>(_f: impl AsRef<T>) {}
fn main() {
    foo::<str>("".to_string());
    foo::<str, String>("".to_string());
}
