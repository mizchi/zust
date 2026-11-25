//! The program tests/check_parallel.rs and tests/check_threads.rs check: shared by both (not a test of its own)

/// A program with enough fns that the first wave is split across children (32 per child), and among them the cases
/// that once lost diagnostics in a child: a fn whose check fails after it requested a closure's instance (the closure
/// is checked only if the parent learns of the request), a fn whose failed lowering told its closure about its
/// regions, an error in a closure's body, and a generic fn's placeholder instance
pub fn program() -> String {
    let mut s = String::new();
    for i in 0..300 {
        s.push_str(&format!("fn f{i}(x: i64) -> i64 {{ let v = vec![x, {i}]; v.iter().map(|y| y * 2).sum::<i64>() + x }}\n"));
    }
    s.push_str(
        r#"
fn a<F>(f: F) -> Result<(), ()> where F: FnMut() { Ok(()) }
fn b() -> i32 { 42 }
fn try_op() -> Result<(), ()> {
    a(|| {
        b()
    })?;
    let x = Ok(42);
    if true {
        x?
    }
    Ok(())
}
pub struct Something;
type Validator<'a> = dyn 'a + Send + Sync + Fn(&'a Something) -> Result<(), ()>;
fn or<'a>(first: &'static Validator<'a>, second: &'static Validator<'a>) -> Validator<'a> {
    return Box::new(move |something: &'_ Something| -> Result<(), ()> {
        first(something).or_else(|_| second(something))
    });
}
fn in_closure() -> i64 {
    let c = |n: i64| -> i64 { n + true };
    c(1)
}
fn generic_ref<T>(x: &T) -> &'static T { x }
fn main() {
    let mut t = 0;
"#,
    );
    for i in 0..300 {
        s.push_str(&format!("    t += f{i}({i});\n"));
    }
    s.push_str("    println!(\"{}\", t + in_closure());\n}\n");
    s
}
