// A trait the program declares, named From: `impl From<S> for S` is that trait's impl, not std's reflexive
// From<S>, so it is no conflict (E0119 is for std's From / TryFrom only)
trait From<T> {
    fn tag(&self, t: &T) -> u8;
}

struct S;

impl From<S> for S {
    fn tag(&self, _: &S) -> u8 {
        7
    }
}

fn main() {
    println!("{}", S.tag(&S));
}
