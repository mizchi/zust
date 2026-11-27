// Three more cases of aliases' defaults and two namespaces' imports (#288's review):
// - a default's free name is the type declared where the alias is, not a generic parameter of the user's of the name
// - a bare alias whose parameters all have defaults (`Bytes` for `type Bytes<T = u8>`)
// - `use facade::Entry;` where facade re-exports, by name, a fn and the struct of a glob of the same name
#[derive(Debug, Default)]
struct Error;
type Pair<T, E = Error> = (T, E);
fn make<Error: Default + std::fmt::Debug>(x: u8) -> (Pair<u8>, Error) {
    ((x, crate::Error), Error::default())
}
type Bytes<T = u8> = Vec<T>;
fn bytes() -> Bytes {
    vec![1, 2]
}
mod api {
    mod inner {
        #[derive(Debug)]
        pub struct Entry(pub u8);
        pub fn entry(x: u8) -> Entry {
            Entry(x)
        }
    }
    pub use self::inner::*;
    #[allow(non_snake_case)]
    pub fn Entry(x: u8) -> u8 {
        x + 1
    }
}
mod facade {
    pub use crate::api::Entry;
}
use facade::Entry;
fn take(e: Entry) -> u8 {
    e.0
}
fn main() {
    let (p, e) = make::<u32>(3);
    println!("{:?} {:?}", p, e);
    println!("{:?}", bytes());
    println!("{} {}", Entry(4), take(api::entry(9)));
}
