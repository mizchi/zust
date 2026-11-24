//! `zustc --test`: the crate's `#[test]` functions, and an entry point that runs them: `#[zust_test_entry] fn
//! __zust_test_entry`, which the program takes as its `main` (sema::program). The crate's own `main`, if it has one,
//! keeps its name and is an ordinary function, as rustc's harness leaves it unused: a test that calls it calls it.
//!
//! The harness follows nextest's model rather than libtest's: it lists its tests and runs the one named, each in a
//! process of its own, so the program needs no `catch_unwind`, threads or output capture. A panic ends the process
//! with 101, a `Result` test that returns `Err` with 1. The command line is libtest's, so `zurgo test` and
//! cargo-nextest can drive it:
//!
//! - `--list [--format terse] [--ignored]`: `name: test` for each test (the ignored ones alone with `--ignored`)
//! - `<filter>.. [--exact] [--skip s]`: the tests to run (substrings, or whole names with `--exact`)
//! - `--ignored` / `--include-ignored`: run the ignored tests only / as well
//! - one test named with `--exact`: it runs in this process (a `#[should_panic]` one in a child, whose exit code and
//!   stderr say whether it panicked as it should)
//! - otherwise each test runs in a child (`/proc/self/exe <name> --exact`), one after another, with libtest's report
//! - `--list --format zust`: also the flags zurgo needs (`ignore`, `should_panic`, the expected text)
//!
//! The source keeps its lines: the harness goes after the last line.

/// A `#[test]` function: its path from the crate root (`tests::adds`), and its attributes
#[derive(Clone, Debug, PartialEq)]
pub struct TestFn {
    pub path: String,
    pub ignore: bool,
    /// `#[should_panic]`: Some(the `expected` text, "" when there is none)
    pub should_panic: Option<String>,
    /// it returns a value (a `Result`): an `Err` fails the test
    pub returns: bool,
}

type Tok = (u8, usize, usize);

/// What `collect` finds in a crate's source
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Collected {
    pub tests: Vec<TestFn>,
    /// `#[test]` attributes that are not on a function collected here: inside a `macro_rules!`, whose expansion comes
    /// later (those tests are not run yet; the build says so)
    pub unlisted: usize,
}

/// The `#[test]` functions of a crate's source (cfg already applied), and where its own `fn main` is
pub fn collect(src: &str) -> Collected {
    let s = src.as_bytes();
    let toks: Vec<Tok> = crate::lexer::tokenize(s);
    let tok = |k: usize| -> &str { toks.get(k).map(|t| &src[t.1..t.2]).unwrap_or("") };
    let sym = |k: usize, c: &str| toks.get(k).is_some_and(|t| t.0 == 1) && tok(k) == c;
    let ident = |k: usize| toks.get(k).is_some_and(|t| t.0 == 0);
    let mut tests = vec![];
    let mut attrs_test = 0usize;
    // (inside a `macro_rules!`'s body: the brace depth outside it)
    let mut macro_end: Option<i32> = None;
    // (the modules open here: (the brace depth inside it, its name)); depth of braces
    let mut mods: Vec<(i32, String)> = vec![];
    let mut depth = 0i32;
    // (a `mod name {` just seen: the next `{` opens it)
    let mut pending_mod: Option<String> = None;
    let mut k = 0;
    while k < toks.len() {
        if sym(k, "{") {
            depth += 1;
            if let Some(n) = pending_mod.take() {
                mods.push((depth, n));
            }
        } else if sym(k, "}") {
            if mods.last().is_some_and(|m| m.0 == depth) {
                mods.pop();
            }
            depth -= 1;
            if macro_end == Some(depth) {
                macro_end = None;
            }
        } else if sym(k, "#") && sym(k + 1, "[") && tok(k + 2) == "test" && sym(k + 3, "]") {
            attrs_test += 1;
        } else if sym(k, "#") && sym(k + 1, "[") && tok(k + 2).starts_with('$') && macro_end.is_some() {
            // (an attribute a macro's caller names, `#[$attr]`: it may be `test`)
            attrs_test += 1;
        } else if ident(k) && tok(k) == "macro_rules" && sym(k + 1, "!") && ident(k + 2) && sym(k + 3, "{") && macro_end.is_none() {
            macro_end = Some(depth);
        } else if ident(k) && tok(k) == "mod" && ident(k + 1) && sym(k + 2, "{") {
            pending_mod = Some(tok(k + 1).trim_start_matches("r#").to_string());
        } else if ident(k) && tok(k) == "fn" && ident(k + 1) && depth == mods.last().map(|m| m.0).unwrap_or(0) {
            let name = tok(k + 1);
            let attrs = attrs_before(src, &toks, k);
            if attrs.iter().any(|a| a.0 == "test") {
                let mut path: Vec<&str> = mods.iter().map(|m| m.1.as_str()).collect();
                path.push(name.trim_start_matches("r#"));
                let should_panic = attrs.iter().find(|a| a.0.starts_with("should_panic")).map(|a| expected(&a.1));
                let ignore = attrs.iter().any(|a| a.0 == "ignore" || a.0.starts_with("ignore="));
                tests.push(TestFn { path: path.join("::"), ignore, should_panic, returns: returns(src, &toks, k + 2) });
            }
        }
        k += 1;
    }
    let unlisted = attrs_test.saturating_sub(tests.len());
    Collected { tests, unlisted }
}

/// The attributes before the item whose keyword `fn` is token k (over `pub`, `pub(..)`, `async`, `unsafe`, `const`,
/// `extern "C"`), without their spaces: `test`, `should_panic(expected="x")`
fn attrs_before(src: &str, toks: &[Tok], k: usize) -> Vec<(String, String)> {
    let tok = |k: usize| -> &str { &src[toks[k].1..toks[k].2] };
    let mut b = k;
    let mut out = vec![];
    loop {
        if b >= 1 && toks[b - 1].0 == 0 && matches!(tok(b - 1), "pub" | "async" | "unsafe" | "const" | "extern" | "default") {
            b -= 1;
        } else if b >= 1 && toks[b - 1].0 == 2 && b >= 2 && tok(b - 2) == "extern" {
            b -= 1;
        } else if b >= 1 && tok(b - 1) == ")" {
            // `pub(crate)`
            let mut j = b - 1;
            while j > 0 && tok(j) != "(" {
                j -= 1;
            }
            if j >= 1 && tok(j - 1) == "pub" {
                b = j - 1;
            } else {
                break;
            }
        } else if b >= 1 && tok(b - 1) == "]" {
            let mut j = b - 1;
            let mut d = 0;
            loop {
                match tok(j) {
                    "]" => d += 1,
                    "[" => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                if j == 0 {
                    return out;
                }
                j -= 1;
            }
            if j >= 1 && tok(j - 1) == "#" {
                // (from its tokens: a comment inside it is gone, a literal stays whole)
                let raw = (j + 1..b - 1).map(|x| tok(x)).collect::<Vec<_>>().join(" ");
                out.push((raw.chars().filter(|c| !c.is_whitespace()).collect(), raw));
                b = j - 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    out
}

/// `should_panic(expected = "x")`'s text ("" without one), from the attribute as written: the literal decoded as
/// Rust decodes it (escapes, raw strings). One that does not decode is a text no panic has, so the test fails
fn expected(attr: &str) -> String {
    let Some(i) = attr.find("expected") else { return String::new() };
    let r = attr[i + 8..].trim_start();
    let Some(r) = r.strip_prefix('=').map(str::trim_start) else { return String::new() };
    str_literal(r).unwrap_or_else(|| format!("\u{0}(the expected text `{}` did not decode)", r.trim_end_matches(')').trim()))
}

/// The value of the string literal at the start of s (`"..."` with escapes, or `r"..."` / `r#"..."#`)
fn str_literal(s: &str) -> Option<String> {
    if let Some(r) = s.strip_prefix('r') {
        let hashes = r.len() - r.trim_start_matches('#').len();
        let body = r[hashes..].strip_prefix('"')?;
        let close = format!("\"{}", "#".repeat(hashes));
        return body.find(&close).map(|j| body[..j].to_string());
    }
    let mut it = s.strip_prefix('"')?.chars().peekable();
    let mut out = String::new();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                'x' => {
                    let h: String = [it.next()?, it.next()?].iter().collect();
                    out.push(char::from(u8::from_str_radix(&h, 16).ok().filter(|v| *v < 0x80)?));
                }
                'u' => {
                    if it.next()? != '{' {
                        return None;
                    }
                    let mut h = String::new();
                    loop {
                        match it.next()? {
                            '}' => break,
                            '_' => {}
                            d => h.push(d),
                        }
                    }
                    out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
                }
                // (a line continuation: the line break and the white space after it)
                '\n' => {
                    while it.peek().is_some_and(|c| c.is_whitespace()) {
                        it.next();
                    }
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// Whether the function whose parameters open at token k returns a value (`-> ..` after its `)`)
fn returns(src: &str, toks: &[Tok], k: usize) -> bool {
    let tok = |k: usize| -> &str { toks.get(k).map(|t| &src[t.1..t.2]).unwrap_or("") };
    // (generics before the parameters)
    let mut k = k;
    if tok(k) == "<" {
        let mut d = 0;
        while k < toks.len() {
            match tok(k) {
                "<" => d += 1,
                ">" => {
                    d -= 1;
                    if d == 0 {
                        k += 1;
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
    }
    if tok(k) != "(" {
        return false;
    }
    let mut d = 0;
    while k < toks.len() {
        match tok(k) {
            "(" => d += 1,
            ")" => {
                d -= 1;
                if d == 0 {
                    break;
                }
            }
            _ => {}
        }
        k += 1;
    }
    tok(k + 1) == "-" && tok(k + 2) == ">" && !(tok(k + 3) == "(" && tok(k + 4) == ")")
}

fn lit(s: &str) -> String {
    format!("{s:?}")
}

/// The source with the harness: `--test`'s crate. Nothing in the source moves: the harness follows the last line
pub fn apply(src: &str) -> String {
    apply_with(src, None, false).0
}

/// apply, with the tests collected from `expanded` when there is one: the same source with its `macro_rules!`
/// expanded (crate::macros), so the tests a macro makes are run too. The harness still goes after the source as it
/// is. Also how many `#[test]` attributes no function was found for (macros, when there is no expansion)
/// (`wasm`: a wasm32 module, which cannot start a child process: a `#[should_panic]` test runs in the module, and the
/// runner (zurgo) checks its panic)
pub fn apply_with(src: &str, expanded: Option<&str>, wasm: bool) -> (String, usize) {
    let c = collect(src);
    let (tests, unlisted) = match expanded {
        Some(e) => (collect(e).tests, 0),
        None => (c.tests.clone(), c.unlisted),
    };
    let mut tests = tests;
    // (libtest's order: by name)
    tests.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out = src.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    let table: Vec<String> = tests
        .iter()
        .map(|t| format!("({}, {}, {})", lit(&t.path), (t.ignore as u8) | ((t.should_panic.is_some() as u8) << 1), lit(t.should_panic.as_deref().unwrap_or(""))))
        .collect();
    let arms: Vec<String> = tests
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if t.returns {
                format!("{i} => {{ if let Err(e) = {}() {{ eprintln!(\"Error: {{:?}}\", e); std::process::exit(1); }} }}", t.path)
            } else {
                format!("{i} => {{ {}(); }}", t.path)
            }
        })
        .collect();
    out.push_str(&format!(
        "#[allow(dead_code)] fn __zust_test_run(i: usize) {{ match i {{ {} _ => {{}} }} }} #[allow(dead_code)] #[zust_test_entry] fn __zust_test_entry() {{ let t: &[(&str, u8, &str)] = &[{}]; let a: Vec<String> = std::env::args().collect(); std::process::exit(__zust_test_main(t, &a)); }} {HARNESS}\n",
        arms.join(" "),
        table.join(", ")
    ));
    let mut out = out.replacen("&& CHILD {", if wasm { "&& false {" } else { "&& true {" }, 1);
    // (on the harness's line: the source's lines stay as they are, and the harness is one more)
    out.pop();
    out.push(' ');
    out.push_str(if wasm { CHILD_FN_WASM } else { CHILD_FN });
    out.push('\n');
    (out, unlisted)
}

/// The harness's child process: a test run again in `/proc/self/exe` (the host only)
const CHILD_FN: &str = concat!(
    "#[allow(dead_code)] fn __zust_test_child(t: &[(&str, u8, &str)], k: usize, ign: bool) -> (bool, String) { ",
    "let mut c = std::process::Command::new(\"/proc/self/exe\"); c.arg(t[k].0); c.arg(\"--exact\"); c.arg(\"--zust-raw\"); if ign { c.arg(\"--include-ignored\"); } ",
    "match c.output() { ",
    "Ok(o) => { let out = String::from_utf8_lossy(&o.stdout); let err = String::from_utf8_lossy(&o.stderr); let code = o.status.code(); ",
    "let ok = if t[k].1 & 2 != 0 { code == Some(101) && match __zust_test_panic_msg(&err) { Some(m) => m.contains(t[k].2), None => false } } else { code == Some(0) }; ",
    "let mut msg = String::new(); msg.push_str(&out); msg.push_str(&err); ",
    "if !ok && t[k].1 & 2 != 0 { if code == Some(101) { msg.push_str(&format!(\"note: panic did not contain expected string\\n      expected substring: {:?}\\n\", t[k].2)); } else { msg.push_str(\"note: test did not panic as expected\\n\"); } } ",
    "(ok, msg) } ",
    "Err(e) => (false, format!(\"cannot run the test: {}\\n\", e)) } } ",
);

/// The same for a wasm32 module, which starts no process (zurgo runs each test in a module of its own)
const CHILD_FN_WASM: &str = "#[allow(dead_code)] fn __zust_test_child(t: &[(&str, u8, &str)], k: usize, ign: bool) -> (bool, String) { let _ = (t, k, ign); (false, String::from(\"a wasm32 module starts no process: run each test with `<module> <name> --exact` (zurgo test does)\\n\")) }";

/// The harness's fixed part (one line: the lines of the source above it stay where they are)
const HARNESS: &str = concat!(
    "#[allow(dead_code)] fn __zust_test_matches(n: &str, pats: &Vec<String>, exact: bool) -> bool { ",
    "for p in pats { if (exact && n == p.as_str()) || (!exact && n.contains(p.as_str())) { return true; } } false } ",
    "#[allow(dead_code)] fn __zust_test_panic_msg(err: &str) -> Option<String> { ",
    "let i = err.rfind(\"' panicked at \")?; let r = &err[i..]; let Some(j) = r.find('\\n') else { return Some(String::new()); }; ",
    "let m = &r[j + 1..]; let m = match m.find(\"\\nnote: run with `RUST_BACKTRACE\") { Some(e) => &m[..e], None => m }; ",
    "let m = match m.find(\"\\nstack backtrace:\") { Some(e) => &m[..e], None => m }; Some(m.to_string()) } ",
    "#[allow(dead_code)] fn __zust_test_main(t: &[(&str, u8, &str)], a: &Vec<String>) -> i32 { ",
    "let mut list = false; let mut exact = false; let mut ignored = false; let mut include = false; let mut raw = false; let mut fmt = String::new(); ",
    "let mut pats: Vec<String> = Vec::new(); let mut skips: Vec<String> = Vec::new(); let mut i = 1; ",
    "while i < a.len() { let x = a[i].as_str(); ",
    "if x == \"--list\" { list = true; } else if x == \"--exact\" { exact = true; } else if x == \"--ignored\" { ignored = true; } ",
    "else if x == \"--include-ignored\" { include = true; } else if x == \"--zust-raw\" { raw = true; } ",
    "else if x == \"--skip\" { i += 1; if i < a.len() { skips.push(a[i].clone()); } } ",
    "else if x == \"--format\" { i += 1; if i < a.len() { fmt = a[i].clone(); } } else if x.starts_with(\"--format=\") { fmt = x[9..].to_string(); } ",
    "else if x == \"--test-threads\" || x == \"--color\" || x == \"--logfile\" || x == \"-Z\" { i += 1; } ",
    "else if !x.starts_with('-') { pats.push(a[i].clone()); } i += 1; } ",
    "let mut sel: Vec<usize> = Vec::new(); let mut filtered = 0; ",
    "for k in 0..t.len() { if (pats.is_empty() || __zust_test_matches(t[k].0, &pats, exact)) && !__zust_test_matches(t[k].0, &skips, exact) { sel.push(k); } else { filtered += 1; } } ",
    "if list { let mut n = 0; for &k in &sel { if ignored && t[k].1 & 1 == 0 { continue; } n += 1; ",
    "if fmt == \"zust\" { println!(\"{}: test{}{}\", t[k].0, if t[k].1 & 1 != 0 { \" ignore\" } else { \"\" }, if t[k].1 & 2 != 0 { format!(\" should_panic {:?}\", t[k].2) } else { String::new() }); } ",
    "else { println!(\"{}: test\", t[k].0); } } ",
    "if fmt != \"terse\" && fmt != \"zust\" { println!(); println!(\"{} tests, 0 benchmarks\", n); } return 0; } ",
    "if raw { for &k in &sel { __zust_test_run(k); } return 0; } ",
    "if exact && sel.len() == 1 { let k = sel[0]; ",
    "if ignored && t[k].1 & 1 == 0 { println!(\"\\nrunning 0 tests\\n\\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; {} filtered out\\n\", filtered + 1); return 0; } ",
    "if t[k].1 & 1 != 0 && !ignored && !include { println!(\"\\nrunning 1 test\\ntest {} ... ignored\\n\\ntest result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; {} filtered out\\n\", t[k].0, filtered); return 0; } ",
    "if t[k].1 & 2 != 0 && CHILD { let (ok, msg) = __zust_test_child(t, k, true); if !ok { eprint!(\"{}\", msg); return 101; } return 0; } ",
    "println!(\"\\nrunning 1 test\"); __zust_test_run(k); println!(\"test {} ... ok\\n\\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; {} filtered out\\n\", t[k].0, filtered); return 0; } ",
    "println!(\"\\nrunning {} tests\", sel.len()); let mut passed = 0; let mut failed: Vec<(usize, String)> = Vec::new(); let mut ign = 0; ",
    "for &k in &sel { let is_ign = t[k].1 & 1 != 0; if (ignored && !is_ign) || (is_ign && !ignored && !include) { if is_ign { println!(\"test {} ... ignored\", t[k].0); ign += 1; } else { filtered += 1; } continue; } ",
    "let (ok, msg) = __zust_test_child(t, k, is_ign); if ok { passed += 1; println!(\"test {} ... ok\", t[k].0); } else { println!(\"test {} ... FAILED\", t[k].0); failed.push((k, msg)); } } ",
    "if !failed.is_empty() { println!(\"\\nfailures:\\n\"); for f in &failed { println!(\"---- {} stdout ----\\n{}\", t[f.0].0, f.1); } println!(\"\\nfailures:\"); for f in &failed { println!(\"    {}\", t[f.0].0); } } ",
    "println!(\"\\ntest result: {}. {} passed; {} failed; {} ignored; 0 measured; {} filtered out\\n\", if failed.is_empty() { \"ok\" } else { \"FAILED\" }, passed, failed.len(), ign, filtered); ",
    "if failed.is_empty() { 0 } else { 101 } }",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_tests() {
        let src = "fn main() {}\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn a() {}\n    #[test]\n    #[ignore = \"slow\"]\n    fn b() {}\n    #[should_panic(expected = \"bo om\")]\n    #[test]\n    fn c() { panic!(\"bo om\") }\n    #[test]\n    fn d() -> Result<(), String> { Ok(()) }\n    fn helper() {}\n    mod inner { #[test] fn e() { fn nested() {} } }\n}\n";
        let c = collect(src);
        let t = c.tests;
        assert_eq!(c.unlisted, 0);
        let names: Vec<&str> = t.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(names, ["tests::a", "tests::b", "tests::c", "tests::d", "tests::inner::e"]);
        assert!(t[1].ignore && !t[0].ignore);
        assert_eq!(t[2].should_panic.as_deref(), Some("bo om"));
        assert!(t[3].returns && !t[0].returns);
        let out = apply(src);
        assert!(out.starts_with(src));
        assert_eq!(out.lines().count(), src.lines().count() + 1);
    }

    #[test]
    fn macro_attrs_counted() {
        let src = "fn main() {}\nmod tests {\n    macro_rules! t { () => { #[test] fn gen() {} } }\n    macro_rules! u { ($a:ident) => { #[$a] fn gen2() {} } }\n}\n";
        assert_eq!(collect(src).unlisted, 2);
        // (with the expansion, the tests it makes are listed)
        let (out, unlisted) = apply_with(src, Some("fn main() {}\nmod tests {\n    #[test] fn gen() {}\n}\n"), false);
        assert_eq!(unlisted, 0);
        assert!(out.contains("(\"tests::gen\", 0, \"\")"), "{out}");
    }

    #[test]
    fn expected_literals() {
        assert_eq!(expected("should_panic(expected = \"a b\")"), "a b");
        assert_eq!(expected("should_panic(expected = r\"wan\\ted\")"), "wan\\ted");
        assert_eq!(expected("should_panic(expected = r#\"q\"uote\"#)"), "q\"uote");
        assert_eq!(expected("should_panic(expected = \"\\u{41}\\x42\\n\")"), "AB\n");
        assert_eq!(expected("should_panic"), "");
        assert!(expected("should_panic(expected = \"\\q\")").starts_with('\u{0}'));
    }
}
