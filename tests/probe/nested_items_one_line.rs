// Items in a fn body on the line of its `{`, of a `;` or of a `}` (#188), in a source with no indented item: moved out
// of the body as indented ones are
fn main() { fn g() -> i32 { 41 } struct P { x: i32 } impl P { fn twice(&self) -> i32 { self.x * 2 } } const K: i32 = 2; let p = P { x: g() }; println!("{} {} {}", p.x + 1, p.twice(), K); }
