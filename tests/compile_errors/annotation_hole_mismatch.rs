// An annotation with holes, of another struct than the value's (no coercion makes one struct another)
use std::marker::PhantomData;

struct Foo<T>(PhantomData<T>);
struct Bar<U>(PhantomData<U>);

fn main() {
    let _ok: Foo<_> = Foo::<u8>(PhantomData);
    let _t: (_, u8) = (1, 2);
    let _x: Foo<_> = Bar::<usize>(PhantomData);
}
