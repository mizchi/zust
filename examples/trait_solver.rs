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
// (two impls of one trait for one type, told apart by the trait's argument)
impl Conv<bool> for u8 {
    fn conv(&self) -> bool {
        *self != 0
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

// (impls for `dyn Tr` and `dyn Tr + Send` do not conflict: a trait object's auto traits make it another type, which
// zustc's Ty does not keep, so E0119 is not decided for trait objects; thiserror has such impls)
trait Erased {
    fn erased(&self) -> u8;
}
impl Erased for dyn std::fmt::Debug {
    fn erased(&self) -> u8 {
        1
    }
}
impl Erased for dyn std::fmt::Debug + Send {
    fn erased(&self) -> u8 {
        2
    }
}

// (associated types normalized by the impl that applies: a binding that holds, a where clause on a projection, and
// the bound a trait puts on its associated type)
trait Container {
    type Elem;
    fn first(&self) -> Self::Elem;
}
struct Bytes(Vec<u8>);
struct Words(Vec<u32>);
impl Container for Bytes {
    type Elem = u8;
    fn first(&self) -> u8 {
        self.0[0]
    }
}
impl Container for Words {
    type Elem = u32;
    fn first(&self) -> u32 {
        self.0[0]
    }
}
fn first_byte<C: Container<Elem = u8>>(c: &C) -> u8 {
    c.first()
}
trait Printable {
    fn show(&self) -> String;
}
impl<C> Printable for C
where
    C: Container,
    C::Elem: std::fmt::Display,
{
    fn show(&self) -> String {
        format!("first {}", self.first())
    }
}
trait Linked {
    type Ok;
    type Next: Container<Elem = Self::Ok>;
}
impl Linked for Bytes {
    type Ok = u32;
    type Next = Words;
}

// (`-> impl Tr` whose body's type holds Tr, with a binding; and a trait's `-> impl Tr` an impl writes concretely)
fn some_bytes() -> impl Container<Elem = u8> {
    Bytes(vec![42])
}
trait Maker {
    fn make() -> impl Container;
}
impl Maker for Words {
    fn make() -> Words {
        Words(vec![5])
    }
}

// (what the old string-based check rejected though rustc accepts it: an impl whose parameters are not in its
// self type's order, a reference to a closure as a Fn, a std blanket impl, and std's impls for ())
struct WithHelper<H, T> {
    helper: H,
    value: T,
}
trait Helper<T> {
    fn get(&self) -> &T;
}
impl<T> Helper<T> for Option<T> {
    fn get(&self) -> &T {
        self.as_ref().unwrap()
    }
}
impl<T, H: Helper<T>> std::ops::Deref for WithHelper<H, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.helper.get()
    }
}
fn call_twice<F: Fn() -> i32>(f: F) -> i32 {
    f() + f()
}
fn borrowed<T: std::borrow::Borrow<T>>(t: T) -> T {
    t
}
fn is_default<T: PartialEq + Default>(a: T) -> bool {
    a == T::default()
}

fn main() {
    println!("{} {}", Local.kind(), Tagged.kind());
    println!("{}", show(&Square(3)));
    println!("{}", show(&Pair(Square(1), Pair(Square(2), Square(3)))));
    println!("{}", same(1u8, 2u8));
    let a: u64 = conv_to(7u8);
    let b: bool = conv_to(0u8);
    let c: u64 = 3u8.conv();
    let d: bool = 3u8.conv();
    println!("{a} {b} {c} {d}");
    let e: bool = Conv::conv(&5u8);
    let f: u64 = Conv::conv(&5u8);
    let g: bool = <u8 as Conv<bool>>::conv(&0u8);
    println!("{e} {f} {g}");
    let b = Bytes(vec![7, 8]);
    println!("{} {} {}", first_byte(&b), b.show(), Words(vec![9]).show());
    println!("{} {}", some_bytes().first(), Words::make().first());
    let w: WithHelper<Option<u8>, u8> = WithHelper { helper: Some(5), value: 2 };
    let k = || 21;
    println!("{} {} {} {} {}", *w, w.value, call_twice(&k), borrowed(3u8), is_default(()));
}
