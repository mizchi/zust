// `'a: while break 'a {}`: a labeled while's label is a target of `break` in its condition too
fn main() {
    let mut n = 0;
    'a: while break 'a {}
    'b: while { n += 1; if n > 3 { break 'b; } true } {
        print!("{n} ");
    }
    'c: while n < 10 {
        n += 1;
        'd: while if n == 7 { break 'c } else { true } {
            break 'd;
        }
    }
    println!("{n}");
}
