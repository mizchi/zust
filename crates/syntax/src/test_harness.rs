//! `zustc --test`: the crate's `#[test]` functions, and a `main` that runs them (the crate's own `main`, if it has
//! one, is renamed out of the way, as rustc's harness leaves it unused).
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
    /// where the crate's own `fn main`'s name is, and the calls of it (`main()`, `super::main()`, `crate::main()`
    /// outside a module with a `main` of its own): renamed together, so a test that calls `main` still calls it
    pub main_refs: Vec<usize>,
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
    let mut main_at = None;
    // (the modules with a `fn main` of their own, by path, and the calls of `main` with their module's path)
    let mut own_main: Vec<String> = vec![];
    // (a call's position, the module whose `main` it names, whether it is a bare `main`)
    let mut calls: Vec<(usize, String, bool)> = vec![];
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
        } else if ident(k) && tok(k) == "main" && sym(k + 1, "(") && !(k >= 1 && (tok(k - 1) == "fn" || sym(k - 1, "."))) {
            // (a call of a `main`: the module its path names, from the module it is in. `crate::`, `self::`,
            //  `super::` (repeated) are followed; a path through another module is not the root's `main`)
            let here: Vec<&str> = mods.iter().map(|m| m.1.as_str()).collect();
            let mut segs: Vec<&str> = vec![];
            let mut j = k;
            while j >= 3 && sym(j - 1, ":") && sym(j - 2, ":") && ident(j - 3) {
                segs.insert(0, tok(j - 3));
                j -= 3;
            }
            let target: Option<Vec<&str>> = if segs.is_empty() {
                // (a bare `main`: this module's own, else the one a `use super::*` brings: taken as the root's)
                Some(here.clone())
            } else {
                let mut t: Vec<&str> = here.clone();
                let mut ok = true;
                for (i, sg) in segs.iter().enumerate() {
                    match *sg {
                        "crate" if i == 0 => t.clear(),
                        "self" if i == 0 => {}
                        "super" => {
                            if t.pop().is_none() {
                                ok = false;
                            }
                        }
                        m => t.push(m),
                    }
                }
                ok.then_some(t)
            };
            if let Some(t) = target {
                calls.push((toks[k].1, t.join("::"), segs.is_empty()));
            }
        } else if ident(k) && tok(k) == "mod" && ident(k + 1) && sym(k + 2, "{") {
            pending_mod = Some(tok(k + 1).trim_start_matches("r#").to_string());
        } else if ident(k) && tok(k) == "fn" && ident(k + 1) && depth == mods.last().map(|m| m.0).unwrap_or(0) {
            let name = tok(k + 1);
            if name == "main" {
                if depth == 0 {
                    main_at = Some(toks[k + 1].1);
                } else {
                    own_main.push(mods.iter().map(|m| m.1.as_str()).collect::<Vec<_>>().join("::"));
                }
            }
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
    let mut main_refs = vec![];
    if let Some(at) = main_at {
        main_refs.push(at);
        // (the calls that name the root's `main`: by a path to the root, or a bare `main` in a module without its own)
        main_refs.extend(calls.into_iter().filter(|(_, m, bare)| m.is_empty() || (*bare && !own_main.contains(m))).map(|c| c.0));
    }
    let unlisted = attrs_test.saturating_sub(tests.len());
    Collected { tests, main_refs, unlisted }
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

/// The source with the harness: `--test`'s crate. The crate's own `fn main` is renamed (same length, so nothing
/// moves) and a `main` that runs the tests follows the last line
pub fn apply(src: &str) -> String {
    apply_noting(src).0
}

/// apply, and how many `#[test]` attributes it found no function for (inside a `macro_rules!`: not run yet)
pub fn apply_noting(src: &str) -> (String, usize) {
    let c = collect(src);
    let unlisted = c.unlisted;
    let mut tests = c.tests;
    // (libtest's order: by name)
    tests.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out = src.to_string();
    for &at in &c.main_refs {
        out.replace_range(at..at + 4, "m41n");
    }
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
        "#[allow(dead_code)] fn __zust_test_run(i: usize) {{ match i {{ {} _ => {{}} }} }} #[allow(dead_code)] fn main() {{ let t: &[(&str, u8, &str)] = &[{}]; let a: Vec<String> = std::env::args().collect(); std::process::exit(__zust_test_main(t, &a)); }} {HARNESS}\n",
        arms.join(" "),
        table.join(", ")
    ));
    (out, unlisted)
}

/// The harness's fixed part (one line: the lines of the source above it stay where they are)
const HARNESS: &str = concat!(
    "#[allow(dead_code)] fn __zust_test_matches(n: &str, pats: &Vec<String>, exact: bool) -> bool { ",
    "for p in pats { if (exact && n == p.as_str()) || (!exact && n.contains(p.as_str())) { return true; } } false } ",
    "#[allow(dead_code)] fn __zust_test_panic_msg(err: &str) -> String { ",
    "let Some(i) = err.rfind(\"' panicked at \") else { return String::new(); }; let r = &err[i..]; let Some(j) = r.find('\\n') else { return String::new(); }; ",
    "let m = &r[j + 1..]; let m = match m.find(\"\\nnote: run with `RUST_BACKTRACE\") { Some(e) => &m[..e], None => m }; ",
    "let m = match m.find(\"\\nstack backtrace:\") { Some(e) => &m[..e], None => m }; m.to_string() } ",
    "#[allow(dead_code)] fn __zust_test_child(t: &[(&str, u8, &str)], k: usize, ign: bool) -> (bool, String) { ",
    "let mut c = std::process::Command::new(\"/proc/self/exe\"); c.arg(t[k].0); c.arg(\"--exact\"); c.arg(\"--zust-raw\"); if ign { c.arg(\"--include-ignored\"); } ",
    "match c.output() { ",
    "Ok(o) => { let out = String::from_utf8_lossy(&o.stdout); let err = String::from_utf8_lossy(&o.stderr); let code = o.status.code(); ",
    "let ok = if t[k].1 & 2 != 0 { code == Some(101) && __zust_test_panic_msg(&err).contains(t[k].2) } else { code == Some(0) }; ",
    "let mut msg = String::new(); msg.push_str(&out); msg.push_str(&err); ",
    "if !ok && t[k].1 & 2 != 0 { if code == Some(101) { msg.push_str(&format!(\"note: panic did not contain expected string\\n      expected substring: {:?}\\n\", t[k].2)); } else { msg.push_str(\"note: test did not panic as expected\\n\"); } } ",
    "(ok, msg) } ",
    "Err(e) => (false, format!(\"cannot run the test: {}\\n\", e)) } } ",
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
    "if t[k].1 & 2 != 0 { let (ok, msg) = __zust_test_child(t, k, true); if !ok { eprint!(\"{}\", msg); return 101; } return 0; } ",
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
        assert_eq!(c.main_refs, [3]);
        assert_eq!(c.unlisted, 0);
        let names: Vec<&str> = t.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(names, ["tests::a", "tests::b", "tests::c", "tests::d", "tests::inner::e"]);
        assert!(t[1].ignore && !t[0].ignore);
        assert_eq!(t[2].should_panic.as_deref(), Some("bo om"));
        assert!(t[3].returns && !t[0].returns);
        let out = apply(src);
        assert!(out.starts_with("fn m41n() {}\n"));
        assert_eq!(out.lines().count(), src.lines().count() + 1);
    }

    #[test]
    fn main_calls_and_macro_tests() {
        let src = "fn main() {}\nmod tests {\n    #[test]\n    fn calls() { main(); super::main(); crate::main(); x.main(); other::main(); }\n    mod m { fn main() {} fn f() { main(); } }\n    macro_rules! t { () => { #[test] fn gen() {} } }\n}\n";
        let c = collect(src);
        // (the declaration, and the three calls of it in `calls`: not the method, `other::main` or m's own)
        assert_eq!(c.main_refs.len(), 4);
        assert!(c.main_refs.iter().all(|&i| &src[i..i + 4] == "main"));
        assert_eq!(c.unlisted, 1);
        let out = apply(src);
        assert!(out.contains("fn calls() { m41n(); super::m41n(); crate::m41n(); x.main(); other::main(); }"), "{out}");
        // (`super::` names the module above: its own `main` is not the root's, and the root's is renamed with the
        //  calls that reach it)
        let src = "fn main() {}\nmod a {\n    fn main() {}\n    fn f() { super::main(); self::main(); main(); }\n    mod b { fn g() { super::main(); super::super::main(); } }\n}\n";
        let out = apply(src);
        assert!(out.contains("fn f() { super::m41n(); self::main(); main(); }"), "{out}");
        assert!(out.contains("fn g() { super::main(); super::super::m41n(); }"), "{out}");
        assert!(out.contains("    fn main() {}"), "{out}");
        // (`crate::main()` from a module with its own `main` still names the root's)
        let src = "fn main() {}\nmod tests {\n    fn main() {}\n    #[test]\n    fn calls_crate_main() { crate::main(); main(); }\n}\n";
        let out = apply(src);
        assert!(out.contains("fn calls_crate_main() { crate::m41n(); main(); }"), "{out}");
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
