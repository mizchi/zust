// W1, after #128: an impl's where clause on another type than a parameter (wasm-bindgen's `impl<T> IntoWasmAbi for
// Vec<T> where Box<[T]>: IntoWasmAbi`, and `#[wasm_bindgen]`'s `impl<T> IntoJsGeneric for JsOption<T> where
// JsOption<T>: JsGeneric`): the trait's supertraits and its associated types' bounds may hold through that clause.
// rustc's output (also with -O)

// (`Vec<T>: D`, I's supertrait, holds through `Box<[T]>: D`, which the where clause `Box<[T]>: I` gives)
trait D { fn d() -> u8; }
trait I: D { type Abi; fn i(&self) -> Self::Abi; }
struct Ws(u8);
impl<T: D> D for Box<[T]> { fn d() -> u8 { T::d() + 10 } }
impl<T> D for Vec<T> where Box<[T]>: D { fn d() -> u8 { <Box<[T]>>::d() + 100 } }
impl<T: D> I for Box<[T]> { type Abi = Ws; fn i(&self) -> Ws { Ws(self.len() as u8) } }
impl<T> I for Vec<T> where Box<[T]>: I<Abi = Ws> { type Abi = Ws; fn i(&self) -> Ws { Ws(self.len() as u8 + 20) } }
impl D for u8 { fn d() -> u8 { 1 } }

// (`type C: Jg` of `O<T>`, which the where clause `O<T>: Jg` gives)
trait Jg {}
trait Ij { type C: Jg; fn c(self) -> u8; }
struct O<T>(T);
impl<T: Jg> Jg for O<T> {}
impl<T> Ij for O<T> where O<T>: Jg { type C = O<T>; fn c(self) -> u8 { 7 } }
struct V;
impl Jg for V {}

fn main() {
    let v = vec![1u8, 2];
    println!("{} {}", v.i().0, <Vec<u8> as D>::d());
    println!("{}", O(V).c());
}
