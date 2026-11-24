struct Foo<T, U = [u8; std::mem::size_of::<T>()]>(T, U);

struct Bar<T> {
    pad: [u8; 16 - core::mem::size_of::<T>()],
}

fn main() {}
