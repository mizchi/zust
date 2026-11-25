//! Whether macros::expand (the fast version: looks at the whole with lexer::tokenize) and the previous version (expand_reference: reads the whole with proc_macro2)
//! give the same results. Compared on the tests, examples and zustc's own sources that contain macro_rules!, and on the compiler combined for self-hosting

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn fast_expand_matches_reference() {
    let mut files = vec![];
    for d in ["tests", "examples", "crates", "out/selfhost"] {
        rs_files(std::path::Path::new(d), &mut files);
    }
    let mut n = 0;
    let mut bad = vec![];
    for p in &files {
        let Ok(src) = std::fs::read_to_string(p) else { continue };
        if !src.contains("macro_rules!") {
            continue;
        }
        n += 1;
        let a = zustc::macros::expand(&src);
        let b = zustc::macros::expand_reference(&src);
        // (Ones where both fail (unreadable sources) are not counted. Different messages are fine if the location is the same)
        match (&a, &b) {
            (Ok(x), Ok(y)) if x == y => {}
            (Err(x), Err(y)) if x.split(':').take(2).eq(y.split(':').take(2)) => {}
            _ => bad.push(format!("{}: {:?}", p.display(), (a.as_ref().map(|o| o.as_ref().map(|s| s.len())), b.as_ref().map(|o| o.as_ref().map(|s| s.len()))))),
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(n >= 5, "{n}");
}
