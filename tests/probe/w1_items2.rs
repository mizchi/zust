// More of what wasm-bindgen 0.2.130's `#[wasm_bindgen]` expansions need (W1 of #38): qualified paths whose self type
// is a reference (`<&JsValue as IntoWasmAbi>::into_abi`) or an associated type
// (`<<&JsValue as IntoWasmAbi>::Abi as WasmAbi>::split`), an impl for a type itself before a blanket one, an
// associated type's value naming another impl's (`type Prim1 = <WasmWord as WasmAbi>::Prim1;` before WasmWord's
// impl), `&[T]`'s impl beside `&mut [u8]`'s, `<Box<[f32]> as FromWasmAbi>::from_abi` beside Box<[u8]>'s, and a
// renamed fn at a range's end (`OFFSET..statics_end()`).

pub struct JsValue(u32);
pub struct WasmRet<T>(T);
pub trait WasmPrimitive {}
impl WasmPrimitive for u32 {}
pub trait WasmAbi {
    type Prim1;
    fn split(self) -> (Self::Prim1, u8, u8, u8);
}
// (a blanket impl whose bound leaves WasmSlice and WasmWord out: their own impls give their Prim1)
impl<T: WasmPrimitive> WasmAbi for T {
    type Prim1 = T;
    fn split(self) -> (T, u8, u8, u8) {
        (self, 0, 0, 0)
    }
}
pub struct WasmSlice {
    ptr: WasmWord,
    len: WasmWord,
}
impl WasmAbi for WasmSlice {
    type Prim1 = <WasmWord as WasmAbi>::Prim1;
    fn split(self) -> (Self::Prim1, u8, u8, u8) {
        (self.ptr.split().0 * 1000 + self.len.split().0, 0, 0, 0)
    }
}
pub struct WasmWord(u32);
impl WasmAbi for WasmWord {
    type Prim1 = u32;
    fn split(self) -> (u32, u8, u8, u8) {
        (self.0, 0, 0, 0)
    }
}
pub trait IntoWasmAbi {
    type Abi: WasmAbi;
    fn into_abi(self) -> Self::Abi;
}
pub trait FromWasmAbi {
    type Abi: WasmAbi;
    unsafe fn from_abi(js: Self::Abi) -> Self;
}
impl<'a> IntoWasmAbi for &'a JsValue {
    type Abi = u32;
    fn into_abi(self) -> u32 {
        self.0
    }
}
impl<'a> IntoWasmAbi for &'a str {
    type Abi = WasmSlice;
    fn into_abi(self) -> WasmSlice {
        WasmSlice { ptr: WasmWord(7), len: WasmWord(self.len() as u32) }
    }
}
impl<T> IntoWasmAbi for *const T {
    type Abi = u32;
    fn into_abi(self) -> u32 {
        5
    }
}
impl FromWasmAbi for bool {
    type Abi = u32;
    unsafe fn from_abi(js: u32) -> bool {
        js % 2 == 1
    }
}
impl<T: WasmAbi> WasmRet<T> {
    fn join(self) -> T {
        self.0
    }
}
// (the import's parameters, as wasm-bindgen writes them)
fn import_is_array(v_1: <<&JsValue as IntoWasmAbi>::Abi as WasmAbi>::Prim1) -> WasmRet<<bool as FromWasmAbi>::Abi> {
    WasmRet(v_1)
}
fn import_str_len(s_1: <<&str as IntoWasmAbi>::Abi as WasmAbi>::Prim1) -> WasmRet<<bool as FromWasmAbi>::Abi> {
    WasmRet(s_1 % 1000)
}
fn is_array(v: &JsValue) -> bool {
    unsafe {
        let _ret = {
            let v = <&JsValue as IntoWasmAbi>::into_abi(v);
            let (v_1, _v_2, _v_3, _v_4) = <<&JsValue as IntoWasmAbi>::Abi as WasmAbi>::split(v);
            import_is_array(v_1)
        };
        <bool as FromWasmAbi>::from_abi(_ret.join())
    }
}
fn odd_len(s: &str) -> bool {
    unsafe {
        let _ret = {
            let s = <&str as IntoWasmAbi>::into_abi(s);
            let (s_1, _s_2, _s_3, _s_4) = <<&str as IntoWasmAbi>::Abi as WasmAbi>::split(s);
            import_str_len(s_1)
        };
        <bool as FromWasmAbi>::from_abi(_ret.join())
    }
}

// (impls for `&mut [u8]` and `&[T]`; for instances of Box<[T]>)
mod slices {
    pub trait IntoAbi {
        fn into_abi(self) -> u32;
    }
    pub trait VectorRef {}
    impl VectorRef for u8 {}
    impl<'a> IntoAbi for &'a mut [u8] {
        fn into_abi(self) -> u32 {
            self.len() as u32 + 100
        }
    }
    impl<'a> IntoAbi for &'a mut [i32] {
        fn into_abi(self) -> u32 {
            self.len() as u32 + 200
        }
    }
    impl<T: VectorRef> IntoAbi for &[T] {
        fn into_abi(self) -> u32 {
            self.len() as u32
        }
    }
    pub trait FromAbi {
        fn from_abi(n: u32) -> Self;
    }
    impl FromAbi for Box<[u8]> {
        fn from_abi(n: u32) -> Self {
            vec![1u8; n as usize].into_boxed_slice()
        }
    }
    impl FromAbi for Box<[f32]> {
        fn from_abi(n: u32) -> Self {
            vec![0.5f32; n as usize + 1].into_boxed_slice()
        }
    }
    pub fn run() {
        let mut v = vec![1u8, 2, 3];
        let a = <&[u8] as IntoAbi>::into_abi(&v);
        let b = <&mut [u8] as IntoAbi>::into_abi(&mut v);
        let x = <Box<[u8]> as FromAbi>::from_abi(2);
        let y = <Box<[f32]> as FromAbi>::from_abi(2);
        println!("{} {} {} {} {} {}", a, b, (&v[..]).into_abi(), x.len(), y.len(), y[0]);
    }
}

// (a renamed fn at a range's end)
mod externref {
    pub fn externref_statics_end() -> u32 {
        9
    }
    pub fn statics_end() -> u32 {
        externref_statics_end()
    }
}
const JSIDX_OFFSET: u32 = 1;
fn externref_statics_end() -> u32 {
    4
}

fn main() {
    println!("{} {} {} {}", is_array(&JsValue(3)), is_array(&JsValue(4)), odd_len("abc"), odd_len("ab"));
    println!("{}", <*const u8 as IntoWasmAbi>::into_abi(core::ptr::null()));
    slices::run();
    let r = JSIDX_OFFSET..externref_statics_end();
    println!("{} {} {}", (JSIDX_OFFSET..externref_statics_end()).contains(&3), r.end, externref::statics_end());
}
