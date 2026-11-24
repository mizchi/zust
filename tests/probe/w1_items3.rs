// More of what wasm-bindgen 0.2.130 needs (W1 of #38): items in `const _: () = { .. };` (what derives and
// `#[wasm_bindgen]` wrap impls in), `<T as Tr>::null()` that is no `ptr::null()`, an impl for a reference to a generic
// struct as a bound, `<Box<[u8]>>::from_abi` among Box's impls, `<T as Tr>::Abi` of a parameter once it is known
// (another trait's `Abi` for the type too), a macro's `let` names beside a field or method of the name and in a
// struct literal's shorthand, `Vec::from_raw_parts`, and `char::from_u32_unchecked`.

pub trait IntoAbi {
    type Abi;
    fn into_abi(self) -> Self::Abi;
}
pub trait Nullable: IntoAbi {
    fn null() -> Self::Abi;
}
pub struct JsValue(u32);
impl IntoAbi for JsValue {
    type Abi = u32;
    fn into_abi(self) -> u32 {
        self.0
    }
}
impl IntoAbi for &JsValue {
    type Abi = u32;
    fn into_abi(self) -> u32 {
        self.0 + 1
    }
}
impl Nullable for JsValue {
    fn null() -> u32 {
        64
    }
}
impl<'a> Nullable for &'a JsValue {
    fn null() -> u32 {
        65
    }
}

// (impls in an anonymous const, with doc comments; a nested one)
pub struct JsClosure {
    obj: JsValue,
}
const _: () = {
    /// The closure's ABI is its JS value's.
    ///
    /// ## Docs
    impl IntoAbi for JsClosure {
        type Abi = <JsValue as IntoAbi>::Abi;
        fn into_abi(self) -> Self::Abi {
            self.obj.into_abi()
        }
    }
    /** by reference */
    impl<'r> IntoAbi for &'r JsClosure {
        type Abi = <&'r JsValue as IntoAbi>::Abi;
        fn into_abi(self) -> Self::Abi {
            (&self.obj).into_abi()
        }
    }
    const _: () = {
        impl Nullable for JsClosure {
            fn null() -> Self::Abi {
                <JsValue as Nullable>::null()
            }
        }
    };
    #[allow(dead_code)]
    struct Section([u8; LEN]);
    const LEN: usize = 3;
};
impl<'r> Nullable for &'r JsClosure {
    fn null() -> Self::Abi {
        <&'r JsValue as Nullable>::null()
    }
}

// (an impl for a reference to a generic struct, as the bound of a call)
pub struct Scoped<T: ?Sized>(Box<T>);
impl<T: ?Sized> IntoAbi for &Scoped<T> {
    type Abi = u32;
    fn into_abi(self) -> u32 {
        9
    }
}
fn assert_abi<T: IntoAbi>(t: T) -> T::Abi {
    t.into_abi()
}

// (`<T as VectorFrom>::Abi` of Box<[T]>'s impl, beside T's IntoAbi::Abi)
pub struct WasmSlice(u32, u32);
pub trait VectorFrom: Sized {
    type Abi;
    fn vector_from_abi(js: Self::Abi) -> Box<[Self]>;
}
impl VectorFrom for u8 {
    type Abi = WasmSlice;
    fn vector_from_abi(js: WasmSlice) -> Box<[u8]> {
        vec![js.0 as u8; js.1 as usize].into_boxed_slice()
    }
}
impl IntoAbi for u8 {
    type Abi = u32;
    fn into_abi(self) -> u32 {
        self as u32
    }
}
pub trait FromAbi {
    type Abi;
    fn from_abi(js: Self::Abi) -> Self;
}
impl FromAbi for Box<[JsValue]> {
    type Abi = u32;
    fn from_abi(js: u32) -> Self {
        vec![JsValue(js)].into_boxed_slice()
    }
}
impl<T: VectorFrom> FromAbi for Box<[T]> {
    type Abi = <T as VectorFrom>::Abi;
    fn from_abi(js: Self::Abi) -> Self {
        T::vector_from_abi(js)
    }
}

// (a macro's `let len` beside `vector.len()` and `js.len`, a struct literal's shorthand, `Vec::from_raw_parts`)
pub struct Raw {
    ptr: usize,
    len: usize,
}
pub struct MutSlice {
    contents: Box<[u8]>,
    js: u32,
}
macro_rules! vectors {
    ($($t:ty)*) => ($(
        impl FromRaw for $t {
            fn to_raw(vector: Box<[$t]>) -> Raw {
                let ptr = vector.as_ptr() as usize;
                let len = vector.len();
                std::mem::forget(vector);
                Raw { ptr, len }
            }
            unsafe fn from_raw(js: Raw) -> Box<[$t]> {
                let ptr = js.ptr as *mut $t;
                let len = js.len;
                unsafe { Vec::from_raw_parts(ptr, len, len) }.into_boxed_slice()
            }
        }
    )*)
}
pub trait FromRaw: Sized {
    fn to_raw(vector: Box<[Self]>) -> Raw;
    unsafe fn from_raw(js: Raw) -> Box<[Self]>;
}
vectors! { u8 i32 }
macro_rules! mut_slice {
    () => {
        fn mut_slice(v: Vec<u8>, idx: u32) -> MutSlice {
            let contents = v.into_boxed_slice();
            let js = idx + 1;
            MutSlice { contents, js }
        }
    };
}
mut_slice!();

fn main() {
    let c = JsClosure { obj: JsValue(3) };
    println!("{} {} {}", <JsClosure as Nullable>::null(), <&JsClosure as Nullable>::null(), (&c).into_abi());
    println!("{} {}", c.into_abi(), assert_abi(&Scoped(Box::new(1u8))));
    let a = <Box<[u8]>>::from_abi(WasmSlice(5, 2));
    let b = <Box<[JsValue]>>::from_abi(7);
    println!("{:?} {} {}", a, b[0].0, 4u8.into_abi());
    let r = <u8 as FromRaw>::to_raw(vec![1u8, 2, 3].into_boxed_slice());
    let back = unsafe { <u8 as FromRaw>::from_raw(r) };
    let m = mut_slice(vec![1, 2], 4);
    println!("{:?} {} {}", back, m.contents.len(), m.js);
    let ch = unsafe { char::from_u32_unchecked(0x41) };
    println!("{} {:?}", ch, char::from_u32(0xD800));
}
