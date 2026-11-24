// `impl From<TokenStream> for proc_macro::TokenStream` (proc-macro2 1.0): the self type is written with a path, so it
// is another type than the crate's own `TokenStream`, not a reflexive `impl From<T> for T` (E0119)
extern crate proc_macro;

pub struct TokenStream(u8);

impl From<TokenStream> for proc_macro::TokenStream {
    fn from(_t: TokenStream) -> Self {
        // (zust has no proc_macro crate to run: the body is never called)
        unimplemented!()
    }
}

fn main() {
    println!("{}", TokenStream(3).0);
}
