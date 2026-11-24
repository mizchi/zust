use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};

fn main() {
    let name = Ident::new("Point", Span::call_site());
    let fields = ["x", "y", "z"].map(|f| format_ident!("{}", f));
    let getters = fields.iter().map(|f| {
        let g = format_ident!("get_{}", f);
        quote! { pub fn #g(&self) -> f64 { self.#f } }
    });
    let ts: TokenStream = quote! {
        pub struct #name { #(pub #fields: f64),* }
        impl #name { #(#getters)* }
    };
    println!("{}", ts);
    println!("tokens {}", ts.clone().into_iter().count());
    let n = 3usize;
    println!("{}", quote! { const N: usize = #n; });
    println!("done");
}
