// Associated consts of one name in impls of one generic trait with other arguments (typenum's `impl ToInt<i8> for Z0`
// beside `impl ToInt<i16> for Z0`), and beside an inherent one of the name: each impl's own, by the qualified path's
// trait arguments, a type parameter's bound, or the type the use expects
trait ToInt<T> {
    fn to_int() -> T;
    const INT: T;
}
struct Z0;
struct P1;
impl ToInt<i8> for Z0 {
    fn to_int() -> i8 {
        Self::INT
    }
    const INT: i8 = 8;
}
impl ToInt<i16> for Z0 {
    fn to_int() -> i16 {
        Self::INT
    }
    const INT: i16 = 16;
}
impl ToInt<i32> for Z0 {
    fn to_int() -> i32 {
        Self::INT
    }
    const INT: i32 = 32;
}
impl ToInt<i16> for P1 {
    fn to_int() -> i16 {
        Self::INT
    }
    const INT: i16 = 116;
}
struct Both;
impl Both {
    const INT: u8 = 1;
}
impl ToInt<i8> for Both {
    fn to_int() -> i8 {
        -1
    }
    const INT: i8 = -1;
}
impl ToInt<i64> for Both {
    fn to_int() -> i64 {
        -64
    }
    const INT: i64 = -64;
}
fn get16<T: ToInt<i16>>() -> i16 {
    T::INT
}
fn get8<T>() -> i8
where
    T: ToInt<i8>,
{
    <T as ToInt<i8>>::INT
}
fn get16_fn<T: ToInt<i16>>() -> i16 {
    T::to_int()
}
fn get32<T: ToInt<i32>>() -> i32 {
    T::INT
}
fn main() {
    println!("{} {} {}", <Z0 as ToInt<i8>>::INT, <Z0 as ToInt<i16>>::INT, <Z0 as ToInt<i32>>::INT);
    let a: i8 = Z0::to_int();
    let b: i16 = Z0::to_int();
    let c: i32 = Z0::to_int();
    println!("{a} {b} {c}");
    let d: i16 = Z0::INT;
    let e: i32 = Z0::INT;
    println!("{d} {e}");
    println!("{} {} {} {}", get16::<Z0>(), get16::<P1>(), get8::<Z0>(), get32::<Z0>());
    println!("{} {}", P1::INT, <P1 as ToInt<i16>>::INT);
    println!("{} {} {}", Both::INT, <Both as ToInt<i8>>::INT, <Both as ToInt<i64>>::INT);
    println!("{}", get8::<Both>());
    println!("{} {} {}", <Z0 as ToInt<i16>>::to_int(), <Z0 as ToInt<i8>>::to_int(), <Z0 as ToInt<i32>>::to_int());
    println!("{} {}", get16_fn::<Z0>(), get16_fn::<P1>());
}
