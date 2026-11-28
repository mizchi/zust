// time-core's units: impls of `MultipleOf<T, Output>` for one type with other arguments, each with its `VALUE`. A
// generic fn's `T::VALUE` is the impl its bound `T: MultipleOf<U, Output>` names, with U and Output as the instance has
// them: two impls with one Output (`MultipleOf<Nano, u16>` and `MultipleOf<Micro, u16>` for Micro) are told apart by U
trait MultipleOf<T, Output> {
    const VALUE: Output;
}
#[derive(Clone, Copy)]
struct Nano;
#[derive(Clone, Copy)]
struct Micro;
#[derive(Clone, Copy)]
struct Milli;
impl MultipleOf<Nano, u8> for Nano {
    const VALUE: u8 = 1;
}
impl MultipleOf<Nano, u16> for Nano {
    const VALUE: u16 = 1;
}
impl MultipleOf<Nano, u16> for Micro {
    const VALUE: u16 = 1_000;
}
impl MultipleOf<Nano, u32> for Micro {
    const VALUE: u32 = 1_000;
}
impl MultipleOf<Nano, f64> for Micro {
    const VALUE: f64 = 1_000.;
}
impl MultipleOf<Micro, u16> for Micro {
    const VALUE: u16 = 1;
}
impl MultipleOf<Micro, u16> for Milli {
    const VALUE: u16 = 1_000;
}
impl MultipleOf<Nano, u32> for Milli {
    const VALUE: u32 = 1_000_000;
}
const fn multiple_of_value<T, U, Output>(_: T) -> Output
where
    T: MultipleOf<U, Output> + Copy,
{
    T::VALUE
}
fn value_of<U, O, T: MultipleOf<U, O>>() -> O {
    <T as MultipleOf<U, O>>::VALUE
}
impl Nano {
    const fn per_t<T: MultipleOf<Self, Output> + Copy, Output>(larger: T) -> Output {
        multiple_of_value::<T, Self, Output>(larger)
    }
}
impl Micro {
    const fn per_t<T, Output>(larger: T) -> Output
    where
        T: MultipleOf<Self, Output> + Copy,
    {
        multiple_of_value::<T, Micro, Output>(larger)
    }
}
fn main() {
    let a: u16 = Nano::per_t(Micro);
    let b: u32 = Nano::per_t(Micro);
    let c: f64 = Nano::per_t(Micro);
    let d: u8 = Nano::per_t(Nano);
    println!("{a} {b} {c} {d}");
    let e: u16 = Micro::per_t(Micro);
    let f: u16 = Micro::per_t(Milli);
    let g: u32 = Nano::per_t(Milli);
    println!("{e} {f} {g}");
    println!("{} {}", <Micro as MultipleOf<Nano, u32>>::VALUE, <Micro as MultipleOf<Micro, u16>>::VALUE);
    println!("{} {}", value_of::<Nano, u16, Micro>(), value_of::<Micro, u16, Micro>());
}
