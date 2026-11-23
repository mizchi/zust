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

/// The `#[test]` functions of a crate's source (cfg already applied), and where its own `fn main`'s name is
pub fn collect(src: &str) -> (Vec<TestFn>, Option<usize>) {
    let s = src.as_bytes();
    let toks: Vec<Tok> = crate::lexer::tokenize(s);
    let tok = |k: usize| -> &str { toks.get(k).map(|t| &src[t.1..t.2]).unwrap_or("") };
    let sym = |k: usize, c: &str| toks.get(k).is_some_and(|t| t.0 == 1) && tok(k) == c;
    let ident = |k: usize| toks.get(k).is_some_and(|t| t.0 == 0);
    let mut tests = vec![];
    let mut main_at = None;
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
        } else if ident(k) && tok(k) == "mod" && ident(k + 1) && sym(k + 2, "{") {
            pending_mod = Some(tok(k + 1).trim_start_matches("r#").to_string());
        } else if ident(k) && tok(k) == "fn" && ident(k + 1) && depth == mods.last().map(|m| m.0).unwrap_or(0) {
            let name = tok(k + 1);
            if depth == 0 && name == "main" {
                main_at = Some(toks[k + 1].1);
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
    (tests, main_at)
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
                let raw = &src[toks[j].2..toks[b - 1].1];
                out.push((raw.chars().filter(|c| !c.is_whitespace()).collect(), raw.to_string()));
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

/// `should_panic(expected = "x")`'s text ("" without one), from the attribute as written
fn expected(attr: &str) -> String {
    let Some(i) = attr.find("expected") else { return String::new() };
    let r = attr[i + 8..].trim_start();
    let Some(r) = r.strip_prefix('=').map(str::trim_start).and_then(|r| r.strip_prefix('"')) else { return String::new() };
    // (to the closing quote: one not after a backslash)
    let mut end = None;
    let mut esc = false;
    for (j, c) in r.char_indices() {
        match c {
            '\\' if !esc => esc = true,
            '"' if !esc => {
                end = Some(j);
                break;
            }
            _ => esc = false,
        }
    }
    end.map(|j| unescape(&r[..j])).unwrap_or_default()
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(x) => out.push(x),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
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
    let (mut tests, main_at) = collect(src);
    // (libtest's order: by name)
    tests.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out = src.to_string();
    if let Some(at) = main_at {
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
    out
}

/// The harness's fixed part (one line: the lines of the source above it stay where they are)
const HARNESS: &str = concat!(
    "#[allow(dead_code)] fn __zust_test_matches(n: &str, pats: &Vec<String>, exact: bool) -> bool { ",
    "for p in pats { if (exact && n == p.as_str()) || (!exact && n.contains(p.as_str())) { return true; } } false } ",
    "#[allow(dead_code)] fn __zust_test_child(t: &[(&str, u8, &str)], k: usize, ign: bool) -> (bool, String) { ",
    "let mut c = std::process::Command::new(\"/proc/self/exe\"); c.arg(t[k].0); c.arg(\"--exact\"); c.arg(\"--zust-raw\"); if ign { c.arg(\"--include-ignored\"); } ",
    "match c.output() { ",
    "Ok(o) => { let out = String::from_utf8_lossy(&o.stdout); let err = String::from_utf8_lossy(&o.stderr); let code = o.status.code(); ",
    "let ok = if t[k].1 & 2 != 0 { code == Some(101) && err.contains(t[k].2) } else { code == Some(0) }; ",
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
        let (t, m) = collect(src);
        assert_eq!(m, Some(3));
        let names: Vec<&str> = t.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(names, ["tests::a", "tests::b", "tests::c", "tests::d", "tests::inner::e"]);
        assert!(t[1].ignore && !t[0].ignore);
        assert_eq!(t[2].should_panic.as_deref(), Some("bo om"));
        assert!(t[3].returns && !t[0].returns);
        let out = apply(src);
        assert!(out.starts_with("fn m41n() {}\n"));
        assert_eq!(out.lines().count(), src.lines().count() + 1);
    }
}
