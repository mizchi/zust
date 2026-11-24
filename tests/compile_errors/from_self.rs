struct S;

impl From<S> for S {
    fn from(s: S) -> S {
        s
    }
}

fn main() {}
