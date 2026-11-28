// A default method the impl for references leaves out runs with Self = &T (#144): `shout` of N = &S is the trait's
// default for `&S`, whose `self.name()` is the impl's, not S's own `shout`. A direct call on `&S` takes S's own
// `shout`; one on `&&S` the default for `&S`. Also a by-value `take(self)` on `&S`: the default for `&S` at the first
// step, before S's own `take(self)`; and a turbofish the impl's method passes on as it is
trait Name {
    fn name(&self) -> String;
    fn shout(&self) -> String {
        format!("{}!", self.name())
    }
    fn take(self) -> String
    where
        Self: Sized,
    {
        format!("take {}", self.name())
    }
}
struct S;
impl Name for S {
    fn name(&self) -> String {
        "S".into()
    }
    fn shout(&self) -> String {
        "S own shout".into()
    }
    fn take(self) -> String {
        "S own take".into()
    }
}
impl<'a, T: ?Sized + Name> Name for &'a T {
    fn name(&self) -> String {
        format!("ref {}", (**self).name())
    }
}
fn show<N: Name>(n: N) -> String {
    n.shout()
}
fn took<N: Name>(n: N) -> String {
    n.take()
}

// (the impl leaves out `size_hint`: a call through the bound with A = &mut S runs the trait's default, 0, not S's 1)
trait Access {
    fn next(&mut self) -> Option<u8>;
    fn size_hint(&self) -> usize {
        0
    }
}
struct A(u8);
impl Access for A {
    fn next(&mut self) -> Option<u8> {
        Some(self.0)
    }
    fn size_hint(&self) -> usize {
        1
    }
}
impl<'a, X: Access> Access for &'a mut X {
    fn next(&mut self) -> Option<u8> {
        (**self).next()
    }
}
fn hint<X: Access>(a: X) -> usize {
    a.size_hint()
}

// (`(**self).size::<7>()`: not a forward of the call's own turbofish, so the impl's method runs: 7, not 3)
trait Size {
    fn size<const N: usize>(&self) -> usize;
}
impl Size for S {
    fn size<const N: usize>(&self) -> usize {
        N
    }
}
impl<'a, T: ?Sized + Size> Size for &'a T {
    fn size<const N: usize>(&self) -> usize {
        (**self).size::<7>()
    }
}
fn get<Z: for<'b> Size>(z: Z) -> usize {
    z.size::<3>()
}

fn main() {
    println!("{}", show(&S));
    println!("{}", show(S));
    println!("{}", (&S).shout());
    println!("{}", (&&S).shout());
    println!("{}", took(&S));
    println!("{}", (&S).take());
    println!("{}", S.take());
    let mut a = A(3);
    println!("{}", hint(&mut a));
    println!("{}", hint(A(4)));
    println!("{}", get(&S));
    println!("{}", get(S));
}
