// A fn's signature is well formed: `Foo<u32>` needs `u32: Trait`
trait Trait {}
struct Foo<T: Trait> {
    x: T,
}
fn explode(x: Foo<u32>) {}
fn main() {}
