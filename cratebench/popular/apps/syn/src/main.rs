use syn::{Item, parse_file};

const SRC: &str = r#"
/// A point.
#[derive(Debug, Clone)]
pub struct Point { pub x: f64, y: f64 }
pub enum Shape { Circle(f64), Rect { w: f64, h: f64 } }
impl Point {
    pub fn norm(&self) -> f64 { (self.x * self.x + self.y * self.y).sqrt() }
}
pub trait Area { fn area(&self) -> f64; }
fn helper<T: Area + ?Sized>(xs: &[Box<T>]) -> f64 { xs.iter().map(|s| s.area()).sum() }
mod inner { pub const N: usize = 3; }
"#;

fn main() {
    let file = parse_file(SRC).expect("parse");
    println!("{} items", file.items.len());
    for it in &file.items {
        let line = match it {
            Item::Struct(s) => format!("struct {} with {} fields", s.ident, s.fields.len()),
            Item::Enum(e) => format!("enum {} with {} variants", e.ident, e.variants.len()),
            Item::Impl(i) => format!("impl with {} items", i.items.len()),
            Item::Trait(t) => format!("trait {} with {} items", t.ident, t.items.len()),
            Item::Fn(f) => format!("fn {} with {} params, {} generics", f.sig.ident, f.sig.inputs.len(), f.sig.generics.params.len()),
            Item::Mod(m) => format!("mod {}", m.ident),
            _ => "other".to_string(),
        };
        println!("{}", line);
    }
    let e: syn::Expr = syn::parse_str("a + b * (c - 1)").expect("expr");
    println!("expr is binary: {}", matches!(e, syn::Expr::Binary(_)));
    // (`a + * b` would parse, as `a + (*b)`: an operator with no right operand cannot)
    let bad = syn::parse_str::<syn::Expr>("a +");
    println!("bad expr rejected: {}", bad.is_err());
    println!("done");
}
