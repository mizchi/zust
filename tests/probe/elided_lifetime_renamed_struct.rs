// A struct with a lifetime parameter, written without it in a fn's parameter, gives the result's elided lifetime
// (`fn block_comment(input: Cursor) -> PResult<&str>` in proc-macro2 1.0). Here two modules have a `Cursor`, so the
// flattened names are `a__Cursor` / `b__Cursor`; the elision must still see the struct's hidden lifetime.
mod a {
    #[derive(Clone, Copy)]
    pub struct Cursor<'s> {
        pub rest: &'s str,
    }
    pub fn word(input: Cursor) -> Option<&str> {
        input.rest.split(' ').next()
    }
}

mod b {
    pub struct Cursor<'s> {
        pub rest: &'s [u8],
    }
    pub fn first(input: Cursor) -> Option<&u8> {
        input.rest.first()
    }
}

// (a name of the program's own with `__` in it is not a module path: `Foo__bar` still has its lifetime)
#[allow(non_camel_case_types)]
struct Foo__bar<'a>(&'a u8);
fn get(x: Foo__bar) -> &u8 {
    x.0
}

fn main() {
    let c = a::Cursor { rest: "hello world" };
    println!("{:?}", a::word(c));
    let d = b::Cursor { rest: &[7, 8] };
    println!("{:?}", b::first(d));
    println!("{}", get(Foo__bar(&3)));
}
