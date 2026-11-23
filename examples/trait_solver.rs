// The trait solver (crates/sema/src/traits.rs) on goals that hold: blanket impls with where clauses, impls that
// chain through each other, trait arguments, and bounds that name the callee's own parameters.

trait Shape {
    fn area(&self) -> u32;
}
trait Named {
    fn name(&self) -> String;
}
trait Describe {
    fn describe(&self) -> String;
}

struct Square(u32);
struct Pair<A, B>(A, B);

impl Shape for Square {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
}
impl Named for Square {
    fn name(&self) -> String {
        "square".to_string()
    }
}
impl<A: Named, B: Named> Named for Pair<A, B> {
    fn name(&self) -> String {
        format!("({}, {})", self.0.name(), self.1.name())
    }
}
impl<A: Shape, B: Shape> Shape for Pair<A, B> {
    fn area(&self) -> u32 {
        self.0.area() + self.1.area()
    }
}
// (a blanket impl whose where clause holds through the other impls)
impl<T> Describe for T
where
    T: Shape + Named,
{
    fn describe(&self) -> String {
        format!("{} of area {}", self.name(), self.area())
    }
}

fn show<T: Describe>(x: &T) -> String {
    x.describe()
}

// (a trait with an argument, and a bound naming the callee's parameter: `U: Same<T>`)
trait Same<T> {}
impl<T> Same<T> for T {}
fn same<T, U: Same<T>>(_: T, u: U) -> U {
    u
}

trait Conv<T> {
    fn conv(&self) -> T;
}
impl Conv<u64> for u8 {
    fn conv(&self) -> u64 {
        *self as u64 * 1000
    }
}
fn conv_to<T, S: Conv<T>>(s: S) -> T {
    s.conv()
}

// (two impls that do not conflict: the blanket one's where clause does not hold for Local, a type of this crate)
trait Marker {}
trait Kind {
    fn kind(&self) -> &'static str;
}
impl<T: Marker> Kind for T {
    fn kind(&self) -> &'static str {
        "marked"
    }
}
struct Local;
struct Tagged;
impl Marker for Tagged {}
impl Kind for Local {
    fn kind(&self) -> &'static str {
        "local"
    }
}

fn main() {
    println!("{} {}", Local.kind(), Tagged.kind());
    println!("{}", show(&Square(3)));
    println!("{}", show(&Pair(Square(1), Pair(Square(2), Square(3)))));
    println!("{}", same(1u8, 2u8));
    let a: u64 = conv_to(7u8);
    println!("{a}");
}
