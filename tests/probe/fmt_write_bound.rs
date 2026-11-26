// String satisfies a generic `W: fmt::Write` bound (#148: the prelude left `impl fmt::Write for String` out unless
// the program named `write_str`): `write!` / `writeln!` into a generic W, the bound alone, and the `fmt::Write` path
// form. (No `write_str` anywhere in this file: naming it hid the bug)
use std::fmt;
use std::fmt::Write;

fn render<W: Write>(w: &mut W) {
    write!(w, "p").unwrap();
}

fn lines<W: fmt::Write>(w: &mut W, n: u32) -> fmt::Result {
    for k in 0..n {
        writeln!(w, "line {k}")?;
    }
    Ok(())
}

fn takes<W: Write>(_w: &mut W) -> usize {
    1
}

fn main() {
    let mut s = String::new();
    render(&mut s);
    println!("{s}");
    let mut t = String::new();
    lines(&mut t, 2).unwrap();
    print!("{t}");
    println!("{}", takes(&mut String::from("x")));
}
