// A macro call among the items of `impl .. for [T; N]`: the `;` of the head's `[T; N]` ends nothing, and the call's
// own `;` goes with it (yoke's `unsafe_complex_yoke_impl!();`, defmt's `default_format!();`). A body's `let ptr` does
// not rename the path `ptr::read` (yoke)
use core::mem::ManuallyDrop;
use core::ptr;

trait Yokeable {
    type Output;
    fn transform_owned(self) -> Self::Output;
    fn size(&self) -> usize;
}

macro_rules! complex_impl {
    () => {
        fn transform_owned(self) -> Self::Output {
            unsafe {
                let ptr: *const Self::Output = (&self as *const Self).cast();
                let _ = ManuallyDrop::new(self);
                ptr::read(ptr)
            }
        }

        fn size(&self) -> usize {
            core::mem::size_of::<Self>()
        }
    };
}

impl<T: Copy, const N: usize> Yokeable for [T; N] {
    type Output = [T; N];
    complex_impl!();
}

impl<T: Copy> Yokeable for (T, [u8; 2]) {
    type Output = (T, [u8; 2]);
    complex_impl!();
}

trait Format {
    fn tag(&self) -> &'static str;
    fn show(&self) -> String;
}

macro_rules! default_format {
    () => {
        fn show(&self) -> String {
            format!("{}:{}", self.tag(), N)
        }
    };
}

impl<T, const N: usize> Format for [T; N] {
    default_format!();

    fn tag(&self) -> &'static str {
        match N {
            0 => "empty",
            1 | 2 => "small",
            _ => "big",
        }
    }
}

fn main() {
    let (x, y) = ([1u16, 2, 3], (7u8, [1u8, 2]));
    println!("{} {}", x.size(), y.size());
    let a = x.transform_owned();
    let b = y.transform_owned();
    println!("{:?} {:?}", a, b);
    println!("{} {} {}", [0u8; 0].show(), [1u8; 2].show(), [2u8; 5].show());
}
