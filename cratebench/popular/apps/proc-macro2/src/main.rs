use proc_macro2::{Delimiter, TokenStream, TokenTree};
use std::str::FromStr;

fn walk(ts: TokenStream, depth: usize, out: &mut Vec<String>) {
    for tt in ts {
        match tt {
            TokenTree::Group(g) => {
                let d = match g.delimiter() { Delimiter::Parenthesis => "()", Delimiter::Brace => "{}", Delimiter::Bracket => "[]", Delimiter::None => "none" };
                out.push(format!("{}group {}", "  ".repeat(depth), d));
                walk(g.stream(), depth + 1, out);
            }
            TokenTree::Ident(i) => out.push(format!("{}ident {}", "  ".repeat(depth), i)),
            TokenTree::Punct(p) => out.push(format!("{}punct {}", "  ".repeat(depth), p.as_char())),
            TokenTree::Literal(l) => out.push(format!("{}literal {}", "  ".repeat(depth), l)),
        }
    }
}

fn main() {
    let ts = TokenStream::from_str("fn add(a: u8, b: u8) -> u8 { a + b } // c\nlet s = \"str\"; [1, 2.5, 'c']").expect("lex");
    let mut out = vec![];
    walk(ts.clone(), 0, &mut out);
    for l in &out { println!("{}", l); }
    println!("round trip: {}", ts);
    println!("bad input rejected: {}", TokenStream::from_str("fn f( {").is_err());
    println!("done");
}
