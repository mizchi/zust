//! `check` in forked children (#101) gives the same diagnostics as in one process.
//!
//! zustc runs as its own process: check_many forks only in a process of one thread, which a test harness is not.
/// A program with enough fns that the first wave is split across children (32 per child), and among them the cases
/// that once lost diagnostics in a child: a fn whose check fails after it requested a closure's instance (the closure
/// is checked only if the parent learns of the request), a fn whose failed lowering told its closure about its
/// regions, an error in a closure's body, and a generic fn's placeholder instance
fn program() -> String {
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

#[test]
fn forked_check_gives_the_same_diagnostics() {
    let dir = std::env::temp_dir().join(format!("zust_check_parallel_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("p.rs");
    std::fs::write(&src, program()).unwrap();
    let run = |jobs: &str, fork_min: Option<&str>| {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_zustc"));
        c.args(["check", "--edition=2021", "-j", jobs]).arg(&src).env_remove("ZUSTC_CHECK_FORK_MIN");
        if let Some(m) = fork_min {
            c.env("ZUSTC_CHECK_FORK_MIN", m);
        }
        let o = c.output().unwrap();
        (o.status.code(), String::from_utf8_lossy(&o.stderr).into_owned())
    };
    let one = run("1", None);
    assert!(one.1.lines().count() >= 4, "{}", one.1);
    // (the default threshold: the first wave's 300 fns go to children)
    assert_eq!(run("4", None), one);
    // (a fork for every wave: the closures', the placeholders')
    assert_eq!(run("4", Some("1")), one);
    let _ = std::fs::remove_dir_all(&dir);
}
