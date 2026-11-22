// `cfg_select!`: a `( .. )` arm keeps its delimiters; `stringify!` keeps a call in its arguments
fn main() {
    let a = 2 * cfg_select! { _ => (1 + 2) };
    let s = stringify!(cfg_select! { _ => { marker } });
    println!("{} {} {}", a, s, cfg_select! { unix => { "u" } _ => { "o" } });
}
