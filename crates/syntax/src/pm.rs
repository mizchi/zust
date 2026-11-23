//! Proc macros, run by the toolchain's proc-macro server (mizchi/zust#29, backend A).
//!
//! A proc-macro crate is a dylib that rustc builds (zurgo does it). zustc does not load it itself: the server that
//! comes with the same toolchain (`rust-analyzer-proc-macro-srv` in rustc's sysroot, built for that rustc's proc_macro
//! ABI) loads it and expands each use. A use is one of:
//!
//! - an attribute (`#[name(..)] item`): the item goes in without that attribute, and its output replaces the item;
//! - a derive (`#[derive(.., Name)] item`): the item goes in without that `#[derive]`, and its output goes after the
//!   item, which keeps its other derives;
//! - a function-like call (`name!(..)`): placed as a `macro_rules!` call's expansion is (macros::expand_with).
//!
//! The protocol is the server's JSON lines (API version 6, also 2 to 5): a request per line, token trees as flat
//! arrays (`FlatTree`). Every span is the call site's: proc-macro output has no hygiene of its own in zustc. A use is
//! found by its name, as zurgo's check found them before: a path (`a::name`) not into std or a tool, or a name
//! brought in by `use` (also renamed). The output is one line (the newlines of the use are kept after it, so the
//! lines after stay where they were).

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::str::FromStr;
use std::sync::Mutex;

/// What every expansion starts with: a comment, so the preprocessing knows that a line holds a proc macro's output
/// (lexer::hoist_nested_items looks for items nested in bodies there, not only at the start of indented lines)
pub const MARK: &str = crate::lexer::PM_MARK;

/// What kind of macro a proc-macro crate exports
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Attr,
    Derive,
    Bang,
}

/// A proc-macro crate a crate can use (directly, or through a library that re-exports its macros)
#[derive(Clone, Debug, PartialEq)]
pub struct Lib {
    /// its crate name (`wasm_bindgen_macro`)
    pub name: String,
    pub dylib: PathBuf,
    /// its macros, as the server lists them
    pub macros: Vec<(String, Kind)>,
}

impl Lib {
    fn has(&self, name: &str, kind: Kind) -> bool {
        self.macros.iter().any(|(n, k)| n == name && *k == kind)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the server

struct Srv {
    _child: Child,
    inp: ChildStdin,
    out: BufReader<ChildStdout>,
    version: u32,
}

/// One server for the process, started on the first use (None: not started; Err: it could not start)
static SRV: Mutex<Option<Result<Srv, String>>> = Mutex::new(None);
/// The macros of each dylib, listed once
/// (by the dylib's path, size and modification time: one rebuilt in place is listed again)
#[allow(clippy::type_complexity)]
static LISTED: Mutex<Vec<((PathBuf, u64, u128), Result<Vec<(String, Kind)>, String>)>> = Mutex::new(Vec::new());

/// The server's path: `ZUSTC_PROC_MACRO_SRV` (empty: none), or `libexec/rust-analyzer-proc-macro-srv` in rustc's
/// sysroot (`RUSTC`, or `rustc`; it comes with the `rustc` component)
pub fn server_path() -> Option<PathBuf> {
    // (set and empty: none, proc macros do not run)
    if let Some(p) = std::env::var_os("ZUSTC_PROC_MACRO_SRV") {
        return (!p.is_empty()).then(|| PathBuf::from(p));
    }
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc).args(["--print", "sysroot"]).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let root = String::from_utf8(out.stdout).ok()?;
    let p = Path::new(root.trim()).join("libexec").join("rust-analyzer-proc-macro-srv");
    p.is_file().then_some(p)
}

fn start() -> Result<Srv, String> {
    let p = server_path().ok_or_else(|| {
        "the toolchain's proc-macro server (`libexec/rust-analyzer-proc-macro-srv` in rustc's sysroot) is not there: proc macros cannot run (set ZUSTC_PROC_MACRO_SRV to its path)".to_string()
    })?;
    let mut child = Command::new(&p)
        // (the server refuses to run without it: it is meant for rust-analyzer, whose protocol this is)
        .env("RUST_ANALYZER_INTERNALS_DO_NOT_USE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", p.display()))?;
    let inp = child.stdin.take().ok_or("the proc-macro server has no stdin")?;
    let out = BufReader::new(child.stdout.take().ok_or("the proc-macro server has no stdout")?);
    let mut s = Srv { _child: child, inp, out, version: 0 };
    let r = s.call("{\"ApiVersionCheck\":{}}")?;
    let v = r.get("ApiVersionCheck").and_then(J::num).ok_or_else(|| format!("the proc-macro server's answer to the version check: {r:?}"))?;
    if v < 2.0 {
        return Err(format!("the proc-macro server speaks version {v} of its protocol: zustc needs 2 or later"));
    }
    s.version = v as u32;
    Ok(s)
}

impl Srv {
    fn call(&mut self, req: &str) -> Result<J, String> {
        let dead = |e: std::io::Error| format!("the proc-macro server: {e}");
        self.inp.write_all(req.as_bytes()).map_err(dead)?;
        self.inp.write_all(b"\n").map_err(dead)?;
        self.inp.flush().map_err(dead)?;
        let mut line = String::new();
        loop {
            line.clear();
            if self.out.read_line(&mut line).map_err(dead)? == 0 {
                return Err("the proc-macro server exited (a proc macro may have crashed it)".into());
            }
            // (a proc macro that prints to stdout prints into the protocol: what is not a message is skipped)
            if line.trim_start().starts_with('{') {
                if let Ok(j) = parse_json(&line) {
                    return Ok(j);
                }
            }
        }
    }
}

/// A request to the server (started if needed; started again after it died)
fn request(req: &str) -> Result<(J, u32), String> {
    let mut g = SRV.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(start());
    }
    let srv = match g.as_mut().unwrap() {
        Ok(s) => s,
        Err(e) => return Err(e.clone()),
    };
    let v = srv.version;
    match srv.call(req) {
        Ok(j) => Ok((j, v)),
        Err(e) => {
            *g = None;
            Err(e)
        }
    }
}

/// The version of the server's protocol (it is started if needed)
pub fn server_version() -> Result<u32, String> {
    let mut g = SRV.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(start());
    }
    match g.as_ref().unwrap() {
        Ok(s) => Ok(s.version),
        Err(e) => Err(e.clone()),
    }
}

/// A proc-macro crate's macros, as the server reads them from its dylib (once per dylib)
pub fn load(name: &str, dylib: &Path) -> Result<Lib, String> {
    let meta = std::fs::metadata(dylib).ok();
    let key = (
        dylib.to_path_buf(),
        meta.as_ref().map_or(0, |m| m.len()),
        meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos()),
    );
    let mut g = LISTED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, r)) = g.iter().find(|(k, _)| *k == key) {
        return r.clone().map(|macros| Lib { name: name.to_string(), dylib: dylib.to_path_buf(), macros });
    }
    let r = list(dylib);
    g.retain(|(k, _)| k.0 != key.0);
    g.push((key, r.clone()));
    r.map(|macros| Lib { name: name.to_string(), dylib: dylib.to_path_buf(), macros })
}

fn list(dylib: &Path) -> Result<Vec<(String, Kind)>, String> {
    let (r, _) = request(&format!("{{\"ListMacros\":{{\"dylib_path\":{}}}}}", js(&dylib.display().to_string())))?;
    let res = r.get("ListMacros").ok_or_else(|| format!("the proc-macro server's answer to ListMacros: {r:?}"))?;
    if let Some(e) = res.get("Err") {
        return Err(format!("the proc macros of {}: {}", dylib.display(), e.str().unwrap_or("")));
    }
    let mut v = vec![];
    for m in res.get("Ok").and_then(J::arr).unwrap_or(&[]) {
        let (Some(n), Some(k)) = (m.at(0).and_then(J::str), m.at(1).and_then(J::str)) else { continue };
        let k = match k {
            "Attr" => Kind::Attr,
            "CustomDerive" => Kind::Derive,
            "FuncLike" | "Bang" => Kind::Bang,
            _ => continue,
        };
        v.push((n.to_string(), k));
    }
    Ok(v)
}

/// One expansion: the macro `name` of `lib` on `body` (and `attr`, an attribute's arguments), in the crate whose
/// compile-time environment is `env`. The output, as source text on one line
pub fn expand(lib: &Lib, name: &str, attr: Option<&str>, body: &str, env: &[(String, String)]) -> Result<String, String> {
    let version = server_version()?;
    let parse = |s: &str| TokenStream::from_str(s).map_err(|e| format!("the input of `{name}` cannot be read as tokens: {e}"));
    let body_ft = flat(parse(body)?, version);
    let attr_ft = match attr {
        Some(a) => flat(parse(a)?, version),
        None => "null".into(),
    };
    let envs: Vec<String> = env.iter().map(|(k, v)| format!("[{},{}]", js(k), js(v))).collect();
    let cwd = match env.iter().rev().find(|(k, _)| k == "CARGO_MANIFEST_DIR") {
        Some((_, d)) => js(d),
        None => "null".into(),
    };
    let lib_path = js(&lib.dylib.display().to_string());
    let req = format!(
        "{{\"ExpandMacro\":{{\"lib\":{lib_path},\"env\":[{}],\"current_dir\":{cwd},\"macro_body\":{body_ft},\"macro_name\":{},\"attributes\":{attr_ft},\"has_global_spans\":{{\"def_site\":0,\"call_site\":0,\"mixed_site\":0}}}}}}",
        envs.join(","),
        js(name)
    );
    let (r, v) = request(&req)?;
    let res = r.get("ExpandMacro").or_else(|| r.get("ExpandMacroExtended")).ok_or_else(|| format!("the proc-macro server's answer to ExpandMacro: {r:?}"))?;
    if let Some(e) = res.get("Err") {
        let msg = e.str().map(str::to_string).or_else(|| e.at(0).and_then(J::str).map(str::to_string)).unwrap_or_else(|| format!("{e:?}"));
        return Err(format!("the proc macro `{name}` panicked: {msg}"));
    }
    let ok = res.get("Ok").ok_or_else(|| format!("the proc-macro server's answer to ExpandMacro: {r:?}"))?;
    // (ExpandMacroExtended: {tree, span_data_table})
    let ft = ok.get("tree").unwrap_or(ok);
    let out = unflat(ft, v)?;
    crate::lexer::PM_EXPANDED.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(format!("{MARK} {out}"))
}

// ---------------------------------------------------------------------------------------------------------------
// FlatTree: subtrees [open, close, kind, first child, end] (close from version 2), literals [span, text, kind, suffix]
// ([span, text] before 5), puncts [span, char, joint], idents [span, text, raw] ([span, text] before 5); the children of
// every subtree in `token_tree` as `index << 2 | tag` (0 subtree, 1 literal, 2 punct, 3 ident), subtrees breadth first

/// A token stream as FlatTree JSON
fn flat(ts: TokenStream, version: u32) -> String {
    let ext = version >= 5;
    let mut subtree: Vec<u32> = vec![];
    let mut literal: Vec<u32> = vec![];
    let mut punct: Vec<u32> = vec![];
    let mut ident: Vec<u32> = vec![];
    let mut tt: Vec<u32> = vec![];
    let mut text: Vec<String> = vec![];
    let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut intern = |s: String, text: &mut Vec<String>| -> u32 {
        *seen.entry(s).or_insert_with_key(|s| {
            text.push(s.clone());
            (text.len() - 1) as u32
        })
    };
    let sub_n = if version >= 2 { 5 } else { 4 };
    let push_sub = |subtree: &mut Vec<u32>, kind: u32| {
        subtree.push(0);
        if sub_n == 5 {
            subtree.push(0);
        }
        subtree.extend([kind, u32::MAX, u32::MAX]);
    };
    push_sub(&mut subtree, 0);
    let mut work: VecDeque<(usize, Vec<TokenTree>)> = VecDeque::new();
    work.push_back((0, ts.into_iter().collect()));
    while let Some((idx, kids)) = work.pop_front() {
        let first = tt.len();
        tt.resize(first + kids.len(), u32::MAX);
        subtree[idx * sub_n + sub_n - 2] = first as u32;
        subtree[idx * sub_n + sub_n - 1] = (first + kids.len()) as u32;
        for (k, t) in kids.into_iter().enumerate() {
            tt[first + k] = match t {
                TokenTree::Group(g) => {
                    let i = subtree.len() / sub_n;
                    let kind = match g.delimiter() {
                        Delimiter::None => 0,
                        Delimiter::Parenthesis => 1,
                        Delimiter::Brace => 2,
                        Delimiter::Bracket => 3,
                    };
                    push_sub(&mut subtree, kind);
                    work.push_back((i, g.stream().into_iter().collect()));
                    (i as u32) << 2
                }
                TokenTree::Literal(l) => {
                    let i = (literal.len() / if ext { 4 } else { 2 }) as u32;
                    let s = l.to_string();
                    if ext {
                        let (kind, a, b, x) = split_lit(&s);
                        let t = intern(s[a..b].to_string(), &mut text);
                        let suffix = if x < s.len() { intern(s[x..].to_string(), &mut text) } else { u32::MAX };
                        literal.extend([0, t, kind as u32, suffix]);
                    } else {
                        let t = intern(s, &mut text);
                        literal.extend([0, t]);
                    }
                    (i << 2) | 1
                }
                TokenTree::Punct(p) => {
                    let i = (punct.len() / 3) as u32;
                    punct.extend([0, p.as_char() as u32, u32::from(p.spacing() == Spacing::Joint)]);
                    (i << 2) | 2
                }
                TokenTree::Ident(id) => {
                    let s = id.to_string();
                    if ext {
                        let i = (ident.len() / 3) as u32;
                        let (raw, name) = match s.strip_prefix("r#") {
                            Some(n) => (1, n.to_string()),
                            None => (0, s),
                        };
                        let t = intern(name, &mut text);
                        ident.extend([0, t, raw]);
                        (i << 2) | 3
                    } else {
                        let i = (ident.len() / 2) as u32;
                        let t = intern(s, &mut text);
                        ident.extend([0, t]);
                        (i << 2) | 3
                    }
                }
            };
        }
    }
    let nums = |v: &[u32]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
    let texts: Vec<String> = text.iter().map(|t| js(t)).collect();
    format!(
        "{{\"subtree\":[{}],\"literal\":[{}],\"punct\":[{}],\"ident\":[{}],\"token_tree\":[{}],\"text\":[{}]}}",
        nums(&subtree),
        nums(&literal),
        nums(&punct),
        nums(&ident),
        nums(&tt),
        texts.join(",")
    )
}

/// A literal's text as (kind, start and end of the part the server keeps as its text, where its suffix starts).
/// Kinds: 0 other, 1 byte, 2 char, 3 integer, 4 float, 5 str, 6 raw str, 7 byte str, 8 raw byte str, 9 C str, 10 raw C
/// str; a raw one has its number of `#`s in the high byte
fn split_lit(s: &str) -> (u16, usize, usize, usize) {
    let b = s.as_bytes();
    let last = |c: u8| b.iter().rposition(|&x| x == c);
    if s.starts_with("b'") {
        return match last(b'\'') {
            Some(e) if e > 1 => (1, 2, e, e + 1),
            _ => (0, 0, s.len(), s.len()),
        };
    }
    if s.starts_with('\'') {
        return match last(b'\'') {
            Some(e) if e > 0 => (2, 1, e, e + 1),
            _ => (0, 0, s.len(), s.len()),
        };
    }
    let (plain, raw, p) = if s.starts_with("br") {
        (7, 8, 1)
    } else if s.starts_with("cr") {
        (9, 10, 1)
    } else if s.starts_with('b') {
        (7, 8, 1)
    } else if s.starts_with('c') {
        (9, 10, 1)
    } else {
        (5, 6, 0)
    };
    let rest = &s[p..];
    if rest.starts_with('"') {
        return match last(b'"') {
            Some(e) if e > p => (plain, p + 1, e, e + 1),
            _ => (0, 0, s.len(), s.len()),
        };
    }
    if let Some(r) = rest.strip_prefix('r') {
        let hashes = r.bytes().take_while(|&c| c == b'#').count();
        if r[hashes..].starts_with('"') {
            let close = format!("\"{}", "#".repeat(hashes));
            let a = p + 1 + hashes + 1;
            return match s.rfind(close.as_str()) {
                Some(e) if e >= a => (raw | ((hashes.min(255) as u16) << 8), a, e, e + close.len()),
                _ => (0, 0, s.len(), s.len()),
            };
        }
    }
    if b.first().is_some_and(u8::is_ascii_digit) {
        let n = b.len();
        let mut i;
        let mut float = false;
        if s.starts_with("0x") || s.starts_with("0o") || s.starts_with("0b") {
            let hex = s.starts_with("0x");
            i = 2;
            while i < n && (b[i] == b'_' || if hex { b[i].is_ascii_hexdigit() } else { b[i].is_ascii_digit() }) {
                i += 1;
            }
        } else {
            i = 0;
            while i < n && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            if i < n && b[i] == b'.' && (i + 1 == n || !(b[i + 1] == b'.' || b[i + 1] == b'_' || b[i + 1].is_ascii_alphabetic())) {
                float = true;
                i += 1;
                while i < n && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
            }
            if i < n && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < n && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                let d = j;
                while j < n && (b[j].is_ascii_digit() || b[j] == b'_') {
                    j += 1;
                }
                if b[d..j].iter().any(u8::is_ascii_digit) {
                    float = true;
                    i = j;
                }
            }
        }
        return (if float { 4 } else { 3 }, 0, i, i);
    }
    (0, 0, s.len(), s.len())
}

/// FlatTree JSON as source text on one line
fn unflat(ft: &J, version: u32) -> Result<String, String> {
    let nums = |k: &str| -> Result<Vec<u32>, String> {
        let a = ft.get(k).and_then(J::arr).ok_or_else(|| format!("proc-macro output: no `{k}`"))?;
        Ok(a.iter().map(|x| x.num().unwrap_or(0.0) as u32).collect())
    };
    let subtree = nums("subtree")?;
    let literal = nums("literal")?;
    let punct = nums("punct")?;
    let ident = nums("ident")?;
    let tt = nums("token_tree")?;
    let text: Vec<&str> = ft.get("text").and_then(J::arr).ok_or("proc-macro output: no `text`")?.iter().map(|x| x.str().unwrap_or("")).collect();
    let ext = version >= 5;
    let sub_n = if version >= 2 { 5 } else { 4 };
    let lit_n = if ext { 4 } else { 2 };
    let id_n = if ext { 3 } else { 2 };
    let txt = |k: u32| -> Result<&str, String> { text.get(k as usize).copied().ok_or_else(|| "proc-macro output: a text index out of range".to_string()) };
    let mut out = String::new();
    // (depth first, with an explicit stack: (subtree, next child), and the closer to write after it)
    if subtree.len() < sub_n {
        return Ok(out);
    }
    let range = |i: usize| -> Result<(u32, usize, usize), String> {
        let s = subtree.get(i * sub_n..i * sub_n + sub_n).ok_or("proc-macro output: a subtree index out of range")?;
        Ok((s[sub_n - 3], s[sub_n - 2] as usize, s[sub_n - 1] as usize))
    };
    let mut stack: Vec<(usize, usize, usize, u32)> = vec![];
    let (k0, lo, hi) = range(0)?;
    stack.push((lo, hi, 0, k0));
    let mut steps = 0usize;
    while let Some(top) = stack.last_mut() {
        steps += 1;
        if steps > 50_000_000 {
            return Err("proc-macro output: a cycle in its token tree".into());
        }
        let (pos, hi) = (top.0 + top.2, top.1);
        if pos >= hi {
            let kind = top.3;
            stack.pop();
            if !stack.is_empty() {
                out.push_str(match kind {
                    1 => ") ",
                    2 => "} ",
                    3 => "] ",
                    _ => " ",
                });
            }
            continue;
        }
        top.2 += 1;
        let x = *tt.get(pos).ok_or("proc-macro output: a child index out of range")?;
        let i = (x >> 2) as usize;
        match x & 3 {
            0 => {
                let (kind, lo, hi) = range(i)?;
                out.push_str(match kind {
                    1 => "(",
                    2 => "{",
                    3 => "[",
                    _ => "",
                });
                stack.push((lo, hi, 0, kind));
            }
            1 => {
                let l = literal.get(i * lit_n..i * lit_n + lit_n).ok_or("proc-macro output: a literal index out of range")?;
                if ext {
                    let suffix = if l[3] == u32::MAX { "" } else { txt(l[3])? };
                    lit_text(&mut out, (l[2] & 0xff) as u8, (l[2] >> 8) as usize, txt(l[1])?, suffix);
                } else {
                    lit_text(&mut out, 0, 0, txt(l[1])?, "");
                }
                out.push(' ');
            }
            2 => {
                let p = punct.get(i * 3..i * 3 + 3).ok_or("proc-macro output: a punct index out of range")?;
                out.push(char::from_u32(p[1]).unwrap_or('?'));
                if p[2] != 1 {
                    out.push(' ');
                }
            }
            _ => {
                let d = ident.get(i * id_n..i * id_n + id_n).ok_or("proc-macro output: an ident index out of range")?;
                if ext && d[2] == 1 {
                    out.push_str("r#");
                }
                out.push_str(txt(d[1])?);
                out.push(' ');
            }
        }
    }
    Ok(out.trim_end().to_string())
}

/// A literal's source text (kind as split_lit gives it). A newline in it is written as `\n` (a raw string with one
/// becomes a plain string), so the output stays on one line
fn lit_text(out: &mut String, kind: u8, hashes: usize, text: &str, suffix: &str) {
    let esc = |t: &str| t.replace('\r', "\\r").replace('\n', "\\n");
    let raw_esc = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"").replace('\r', "\\r").replace('\n', "\\n");
    match kind {
        1 => out.push_str(&format!("b'{}'", esc(text))),
        2 => out.push_str(&format!("'{}'", esc(text))),
        3 | 4 => out.push_str(text),
        5 => out.push_str(&format!("\"{}\"", esc(text))),
        7 => out.push_str(&format!("b\"{}\"", esc(text))),
        9 => out.push_str(&format!("c\"{}\"", esc(text))),
        6 | 8 | 10 => {
            let p = match kind {
                6 => "",
                8 => "b",
                _ => "c",
            };
            if text.contains(['\n', '\r']) {
                out.push_str(&format!("{p}\"{}\"", raw_esc(text)));
            } else {
                let h = "#".repeat(hashes);
                out.push_str(&format!("{p}r{h}\"{text}\"{h}"));
            }
        }
        _ => out.push_str(&esc(text)),
    }
    out.push_str(suffix);
}

// ---------------------------------------------------------------------------------------------------------------
// finding the uses

/// Paths into std (`std::fmt::Debug`), and tools' attributes (`#[rustfmt::skip]`): never a proc macro's
fn std_or_tool(s: &str) -> bool {
    matches!(s, "std" | "core" | "alloc" | "rustfmt" | "clippy" | "rustdoc" | "rust_analyzer" | "diagnostic" | "miri")
}

/// Attributes of the language, never a proc macro's when written without a path
const BUILTIN_ATTRS: &[&str] = &[
    "allow", "automatically_derived", "bench", "cfg", "cfg_attr", "cold", "coverage", "crate_name", "crate_type", "deny", "deprecated", "derive", "doc",
    "expect", "export_name", "feature", "forbid", "global_allocator", "ignore", "inline", "instruction_set", "link", "link_name", "link_ordinal",
    "link_section", "macro_export", "macro_use", "must_use", "naked", "no_main", "no_mangle", "no_std", "non_exhaustive", "panic_handler", "path",
    "proc_macro", "proc_macro_attribute", "proc_macro_derive", "recursion_limit", "repr", "should_panic", "target_feature", "test", "test_case",
    "track_caller", "type_length_limit", "unsafe", "used", "warn", "windows_subsystem",
];

/// Derives of the language: a proc macro's only when the crate imports one of that name (`use derive_more::Debug;`)
const BUILTIN_DERIVES: &[&str] = &["Clone", "Copy", "Debug", "Default", "Eq", "Hash", "Ord", "PartialEq", "PartialOrd"];

fn plain(s: &str) -> &str {
    s.strip_prefix("r#").unwrap_or(s)
}

/// What the crate's `use`s bring in (from outside std): names, and renames (`use a::B as C;`: (B, C))
pub struct Scope<'a> {
    libs: &'a [Lib],
    imported: Vec<String>,
    renames: Vec<(String, String)>,
    /// (a name brought in, the crate its `use` starts from: `use b::make;` gives ("make", "b"))
    origins: Vec<(String, String)>,
}

type Toks = [(u8, usize, usize)];

impl<'a> Scope<'a> {
    pub fn new(libs: &'a [Lib], b: &[u8], toks: &Toks) -> Scope<'a> {
        let tk = |k: usize| -> &[u8] { toks.get(k).map_or(&[][..], |t| &b[t.1..t.2]) };
        let s = |k: usize| String::from_utf8_lossy(tk(k)).into_owned();
        let mut imported = vec![];
        let mut renames = vec![];
        let mut origins = vec![];
        let n = toks.len();
        for k in 0..n {
            if !(toks[k].0 == 0 && tk(k) == b"use") {
                continue;
            }
            let mut j = k + 1;
            if tk(j) == b":" && tk(j + 1) == b":" {
                j += 2;
            }
            if std_or_tool(&s(j)) {
                continue;
            }
            let root = plain(&s(j)).to_string();
            while j < n && tk(j) != b";" {
                if toks[j].0 == 0 {
                    let name = s(j);
                    if tk(j + 1) == b"as" && toks.get(j + 2).is_some_and(|t| t.0 == 0) {
                        let alias = s(j + 2);
                        imported.push(plain(&alias).to_string());
                        renames.push((plain(&name).to_string(), plain(&alias).to_string()));
                        origins.push((plain(&alias).to_string(), root.clone()));
                        j += 3;
                        continue;
                    }
                    if matches!(tk(j + 1), b"," | b";" | b"}") {
                        imported.push(plain(&name).to_string());
                        origins.push((plain(&name).to_string(), root.clone()));
                    }
                }
                j += 1;
            }
        }
        Scope { libs, imported, renames, origins }
    }

    /// The macro a path names (its names in order, `r#` kept off): (the library, the macro's own name)
    pub fn resolve(&self, segs: &[&str], kind: Kind) -> Option<(&'a Lib, String)> {
        let last = plain(segs.last()?);
        if segs.len() > 1 {
            if std_or_tool(segs[0]) && !self.renames.iter().any(|(_, a)| a == segs[0]) {
                return None;
            }
        } else if (kind == Kind::Attr && BUILTIN_ATTRS.contains(&last)) || (kind == Kind::Derive && BUILTIN_DERIVES.contains(&last) && !self.imported.iter().any(|x| x == last)) {
            return None;
        }
        // (the name, and the names it renames, to their end)
        let mut names = vec![last.to_string()];
        let mut k = 0;
        while k < names.len() {
            for (orig, alias) in &self.renames {
                if *alias == names[k] && !names.contains(orig) {
                    names.push(orig.clone());
                }
            }
            k += 1;
        }
        // (a path from a proc-macro crate's own name first: `wasm_bindgen_macro::wasm_bindgen`; a name imported from
        // one, `use b::make;`, is that crate's)
        let first = if segs.len() > 1 {
            Some(plain(segs[0]).to_string())
        } else {
            self.origins.iter().find(|(n, _)| *n == last).map(|(_, r)| r.clone())
        };
        let libs = self.libs.iter().filter(|l| first.as_deref() == Some(l.name.as_str())).chain(self.libs.iter());
        for l in libs {
            if let Some(n) = names.iter().find(|n| l.has(n, kind)) {
                return Some((l, n.clone()));
            }
        }
        None
    }
}

/// The attribute and derive uses in a text (the outermost of each item: what an expansion gives is read again later),
/// expanded: (start, end, output) for each, in order. The arguments of macro calls are not read (their expansion is)
pub fn expand_items(text: &str, libs: &[Lib], env: &[(String, String)]) -> Result<Vec<(usize, usize, String)>, String> {
    let mut reps = vec![];
    if libs.is_empty() || !text.contains("#") {
        return Ok(reps);
    }
    let b = text.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let n = toks.len();
    let (close, depth) = crate::macros::brackets_of(b, &toks);
    let scope = Scope::new(libs, b, &toks);
    let tk = |k: usize| -> &[u8] { toks.get(k).map_or(&[][..], |t| &b[t.1..t.2]) };
    let sym = |k: usize, c: u8| k < n && toks[k].0 == 1 && b[toks[k].1] == c;
    let opens = |k: usize| k < n && toks[k].0 == 1 && matches!(b[toks[k].1], b'(' | b'[' | b'{') && close[k] != usize::MAX;
    // (the `#` of the attribute each `]` closes, when it is an outer attribute)
    let attr_open = |e: usize| -> Option<usize> {
        if !sym(e, b']') {
            return None;
        }
        let o = (0..e).rev().find(|&o| close[o] == e)?;
        (o >= 1 && sym(o - 1, b'#')).then_some(o - 1)
    };
    let mut k = 0;
    while k < n {
        // (a macro call's arguments: `m!(..)`, and a definition `macro_rules! m { .. }`)
        if toks[k].0 == 0 && sym(k + 1, b'!') {
            let g = if tk(k) == b"macro_rules" && toks.get(k + 2).is_some_and(|t| t.0 == 0) { k + 3 } else { k + 2 };
            if opens(g) {
                k = close[g] + 1;
                continue;
            }
        }
        if !(sym(k, b'#') && sym(k + 1, b'[') && close[k + 1] != usize::MAX) {
            k += 1;
            continue;
        }
        let ae = close[k + 1];
        // (the attribute's path)
        let mut j = k + 2;
        if sym(j, b':') && sym(j + 1, b':') {
            j += 2;
        }
        let mut segs: Vec<String> = vec![];
        loop {
            let raw = tk(j) == b"r" && sym(j + 1, b'#') && toks.get(j + 2).is_some_and(|t| t.0 == 0);
            if raw {
                j += 2;
            }
            if toks.get(j).is_none_or(|t| t.0 != 0) {
                break;
            }
            segs.push(String::from_utf8_lossy(tk(j)).into_owned());
            j += 1;
            if sym(j, b':') && sym(j + 1, b':') {
                j += 2;
            } else {
                break;
            }
        }
        let seg_refs: Vec<&str> = segs.iter().map(String::as_str).collect();
        let is_derive = seg_refs == ["derive"] && sym(j, b'(');
        let found = if is_derive { None } else { scope.resolve(&seg_refs, Kind::Attr) };
        // (the derives of a `#[derive(..)]`: (start byte, end byte, path, the proc macro))
        let mut derives: Vec<(usize, usize, Option<(&Lib, String)>)> = vec![];
        if is_derive {
            let ce = close[j];
            let mut d = j + 1;
            while d < ce {
                let st = d;
                let mut ds: Vec<&str> = vec![];
                if sym(d, b':') && sym(d + 1, b':') {
                    d += 2;
                }
                while d < ce && toks[d].0 == 0 {
                    ds.push(std::str::from_utf8(tk(d)).unwrap_or(""));
                    d += 1;
                    if sym(d, b':') && sym(d + 1, b':') {
                        d += 2;
                    } else {
                        break;
                    }
                }
                let en = d;
                while d < ce && !sym(d, b',') {
                    d += 1;
                }
                if en > st {
                    derives.push((toks[st].1, toks[en - 1].2, scope.resolve(&ds, Kind::Derive)));
                }
                d += 1;
            }
        }
        if found.is_none() && !derives.iter().any(|d| d.2.is_some()) {
            k = ae + 1;
            continue;
        }
        // (a derive with an attribute macro after it on the same item: the attribute first, its output has the derive)
        if found.is_none() {
            let mut q = ae + 1;
            let mut later = false;
            while sym(q, b'#') && sym(q + 1, b'[') && close[q + 1] != usize::MAX {
                let mut segs: Vec<&str> = vec![];
                let mut x = q + 2;
                while toks.get(x).is_some_and(|t| t.0 == 0) {
                    segs.push(std::str::from_utf8(tk(x)).unwrap_or(""));
                    x += 1;
                    if sym(x, b':') && sym(x + 1, b':') {
                        x += 2;
                    } else {
                        break;
                    }
                }
                if segs != ["derive"] && scope.resolve(&segs, Kind::Attr).is_some() {
                    later = true;
                    break;
                }
                q = close[q + 1] + 1;
            }
            if later {
                k = ae + 1;
                continue;
            }
        }
        // (the item: from its first attribute (and the doc comments before it) to its end)
        let mut first = k;
        while first >= 1 {
            match attr_open(first - 1) {
                Some(o) => first = o,
                None => break,
            }
        }
        let start = crate::cfg::outer_docs_start(b, toks[first].1);
        let Some(end) = item_end(b, &toks, &close, &depth, ae + 1) else {
            // (not on an item (a statement, a field, an expression): left as it is)
            k = ae + 1;
            continue;
        };
        let (a, e) = (toks[k].1, toks[ae].2);
        // (an error at the use: `line:col: ..`)
        let line = b[..a].iter().filter(|&&c| c == b'\n').count() + 1;
        let col = text[b[..a].iter().rposition(|&c| c == b'\n').map_or(0, |x| x + 1)..a].chars().count() + 1;
        let here = |e: String| format!("{line}:{col}: {e}");
        let iend = toks[end].2;
        let mut input = String::with_capacity(iend - start);
        input.push_str(&text[start..a]);
        input.push_str(&text[e..iend]);
        let out = if let Some((lib, name)) = found {
            // (the arguments: `#[m(..)]`, `#[m]`; `#[m = ..]` gives what follows `=`)
            let args = if j < ae && opens(j) && close[j] + 1 == ae {
                &text[toks[j].2..toks[close[j]].1]
            } else if sym(j, b'=') {
                &text[toks[j].2..toks[ae].1]
            } else {
                ""
            };
            expand(lib, &name, Some(args), &input, env).map_err(here)?
        } else {
            // (the item keeps the derives that are not proc macros'; the derives' output goes after it)
            let keep: Vec<&str> = derives.iter().filter(|d| d.2.is_none()).map(|d| &text[d.0..d.1]).collect();
            let mut s = String::new();
            s.push_str(&text[start..a]);
            if !keep.is_empty() {
                s.push_str(&format!("#[derive({})]", keep.join(", ")));
            }
            s.push_str(&text[e..iend]);
            // (on one line: comments in the item would comment out the rest of the line once its line breaks go, so
            // then the item goes as tokens, read from its lines)
            let mut s = if s.contains("//") || s.contains("/*") { one_line(&s).map_err(here)? } else { s.replace(['\n', '\r'], " ") };
            for (_, _, d) in &derives {
                if let Some((lib, name)) = d {
                    s.push(' ');
                    s.push_str(&expand(lib, name, None, &input, env).map_err(here)?);
                }
            }
            s
        };
        reps.push((start, iend, out));
        k = end + 1;
    }
    Ok(reps)
}

/// Source text on one line, its comments gone (doc comments as `#[doc = ..]`)
fn one_line(s: &str) -> Result<String, String> {
    let ts = TokenStream::from_str(s).map_err(|e| format!("cannot be read as tokens: {e}"))?;
    // (the tokens as the server would give them back: through FlatTree and back)
    let v = 6;
    let ft = parse_json(&flat(ts, v))?;
    unflat(&ft, v)
}

/// The last token of the item that starts at token s (after its attributes), or None when it is not an item
fn item_end(b: &[u8], toks: &Toks, close: &[usize], depth: &[u32], s: usize) -> Option<usize> {
    let n = toks.len();
    let tk = |k: usize| -> &[u8] { toks.get(k).map_or(&[][..], |t| &b[t.1..t.2]) };
    let sym = |k: usize, c: u8| k < n && toks[k].0 == 1 && b[toks[k].1] == c;
    let d0 = *depth.get(s)?;
    let mut j = s;
    // (more outer attributes: `#[a] #[pm] #[b] item`)
    while sym(j, b'#') && sym(j + 1, b'[') && close[j + 1] != usize::MAX {
        j = close[j + 1] + 1;
    }
    if tk(j) == b"pub" {
        j += 1;
        if sym(j, b'(') && close[j] != usize::MAX {
            j = close[j] + 1;
        }
    }
    let mut q = j;
    loop {
        if matches!(tk(q), b"default" | b"unsafe" | b"safe" | b"async" | b"auto") || (tk(q) == b"const" && matches!(tk(q + 1), b"fn" | b"unsafe" | b"async" | b"extern")) {
            q += 1;
        } else if tk(q) == b"extern" && toks.get(q + 1).is_some_and(|t| t.0 == 2) {
            // (`extern "C" fn`, `extern "C" { .. }`)
            q += 2;
        } else {
            break;
        }
    }
    let kw = tk(q);
    let semi_only = matches!(kw, b"const" | b"static" | b"type" | b"use") || (kw == b"extern" && tk(q + 1) == b"crate");
    if !matches!(kw, b"fn" | b"struct" | b"enum" | b"union" | b"impl" | b"trait" | b"mod" | b"extern" | b"const" | b"static" | b"type" | b"use") && !(q > j && sym(q, b'{')) {
        return None;
    }
    let mut k = q;
    // (the depth of `<..>`: a `{ .. }` in generic arguments, `-> Array<{ 1 }>`, is a const argument, not the body; the
    // `>` of `->` / `=>` closes none)
    let mut angle = 0i32;
    while k < n {
        if toks[k].0 == 1 {
            match b[toks[k].1] {
                b'<' if depth[k] == d0 => angle += 1,
                b'>' if depth[k] == d0 && !(k > 0 && toks[k - 1].0 == 1 && matches!(b[toks[k - 1].1], b'-' | b'=') && toks[k - 1].2 == toks[k].1) => {
                    angle = (angle - 1).max(0)
                }
                b';' if depth[k] == d0 => return Some(k),
                b'{' if depth[k] == d0 && !semi_only && angle == 0 => return (close[k] != usize::MAX).then_some(close[k]),
                b'(' | b'[' | b'{' if close[k] != usize::MAX => {
                    k = close[k] + 1;
                    continue;
                }
                b')' | b']' | b'}' => return None,
                _ => {}
            }
        }
        k += 1;
    }
    None
}

// ---------------------------------------------------------------------------------------------------------------
// JSON (what the protocol needs)

#[derive(Clone, Debug, PartialEq)]
enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    fn get(&self, k: &str) -> Option<&J> {
        match self {
            J::Obj(v) => v.iter().find(|(x, _)| x == k).map(|(_, j)| j),
            _ => None,
        }
    }
    fn at(&self, i: usize) -> Option<&J> {
        match self {
            J::Arr(v) => v.get(i),
            _ => None,
        }
    }
    fn arr(&self) -> Option<&[J]> {
        match self {
            J::Arr(v) => Some(v),
            _ => None,
        }
    }
    fn str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            J::Num(x) => Some(*x),
            _ => None,
        }
    }
}

/// A JSON string
fn js(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn parse_json(s: &str) -> Result<J, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i, 0)?;
    ws(b, &mut i);
    if i != b.len() {
        return Err(format!("JSON: trailing data at byte {i}"));
    }
    Ok(v)
}

fn ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize, d: usize) -> Result<J, String> {
    if d > 256 {
        return Err("JSON: nested too deep".into());
    }
    ws(b, i);
    let err = |i: usize| format!("JSON: unexpected input at byte {i}");
    match b.get(*i) {
        Some(b'{') => {
            *i += 1;
            let mut v = vec![];
            ws(b, i);
            if b.get(*i) == Some(&b'}') {
                *i += 1;
                return Ok(J::Obj(v));
            }
            loop {
                ws(b, i);
                let J::Str(k) = value(b, i, d + 1)? else { return Err(err(*i)) };
                ws(b, i);
                if b.get(*i) != Some(&b':') {
                    return Err(err(*i));
                }
                *i += 1;
                let x = value(b, i, d + 1)?;
                v.push((k, x));
                ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b'}') => {
                        *i += 1;
                        return Ok(J::Obj(v));
                    }
                    _ => return Err(err(*i)),
                }
            }
        }
        Some(b'[') => {
            *i += 1;
            let mut v = vec![];
            ws(b, i);
            if b.get(*i) == Some(&b']') {
                *i += 1;
                return Ok(J::Arr(v));
            }
            loop {
                v.push(value(b, i, d + 1)?);
                ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b']') => {
                        *i += 1;
                        return Ok(J::Arr(v));
                    }
                    _ => return Err(err(*i)),
                }
            }
        }
        Some(b'"') => {
            *i += 1;
            let mut s: Vec<u8> = vec![];
            loop {
                match b.get(*i) {
                    None => return Err("JSON: unterminated string".into()),
                    Some(b'"') => {
                        *i += 1;
                        break;
                    }
                    Some(b'\\') => {
                        let c = *b.get(*i + 1).ok_or("JSON: unterminated string")?;
                        *i += 2;
                        match c {
                            b'n' => s.push(b'\n'),
                            b't' => s.push(b'\t'),
                            b'r' => s.push(b'\r'),
                            b'b' => s.push(8),
                            b'f' => s.push(12),
                            b'u' => {
                                let hex = |i: usize| -> Result<u32, String> {
                                    let h = b.get(i..i + 4).and_then(|h| std::str::from_utf8(h).ok()).ok_or("JSON: bad \\u escape")?;
                                    u32::from_str_radix(h, 16).map_err(|_| "JSON: bad \\u escape".to_string())
                                };
                                let mut cp = hex(*i)?;
                                *i += 4;
                                if (0xd800..0xdc00).contains(&cp) && b.get(*i) == Some(&b'\\') && b.get(*i + 1) == Some(&b'u') {
                                    let lo = hex(*i + 2)?;
                                    if (0xdc00..0xe000).contains(&lo) {
                                        cp = 0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00);
                                        *i += 6;
                                    }
                                }
                                let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                                let mut buf = [0u8; 4];
                                s.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            }
                            c => s.push(c),
                        }
                    }
                    Some(&c) => {
                        s.push(c);
                        *i += 1;
                    }
                }
            }
            String::from_utf8(s).map(J::Str).map_err(|_| "JSON: a string that is not UTF-8".to_string())
        }
        Some(b't') if b[*i..].starts_with(b"true") => {
            *i += 4;
            Ok(J::Bool(true))
        }
        Some(b'f') if b[*i..].starts_with(b"false") => {
            *i += 5;
            Ok(J::Bool(false))
        }
        Some(b'n') if b[*i..].starts_with(b"null") => {
            *i += 4;
            Ok(J::Null)
        }
        Some(c) if c.is_ascii_digit() || *c == b'-' => {
            let st = *i;
            *i += 1;
            while *i < b.len() && (b[*i].is_ascii_digit() || matches!(b[*i], b'.' | b'e' | b'E' | b'+' | b'-')) {
                *i += 1;
            }
            std::str::from_utf8(&b[st..*i]).ok().and_then(|t| t.parse::<f64>().ok()).map(J::Num).ok_or_else(|| err(st))
        }
        _ => Err(err(*i)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_split_as_the_server_keeps_them() {
        fn t(s: &str) -> (u16, &str, &str) {
            let (k, a, e, x) = split_lit(s);
            (k, &s[a..e], &s[x..])
        }
        assert_eq!(t("\"a\\\"b\""), (5, "a\\\"b", ""));
        assert_eq!(t("r#\"x\"y\"#"), (6 | (1 << 8), "x\"y", ""));
        assert_eq!(t("b\"x\"suf"), (7, "x", "suf"));
        assert_eq!(t("br##\"x\"##"), (8 | (2 << 8), "x", ""));
        assert_eq!(t("c\"x\""), (9, "x", ""));
        assert_eq!(t("'a'"), (2, "a", ""));
        assert_eq!(t("b'\\n'"), (1, "\\n", ""));
        assert_eq!(t("1u8"), (3, "1", "u8"));
        assert_eq!(t("0xffu8"), (3, "0xff", "u8"));
        assert_eq!(t("0x1f32"), (3, "0x1f32", ""));
        assert_eq!(t("1.5f64"), (4, "1.5", "f64"));
        assert_eq!(t("1e10"), (4, "1e10", ""));
        assert_eq!(t("1."), (4, "1.", ""));
        assert_eq!(t("2_000"), (3, "2_000", ""));
        let mut o = String::new();
        lit_text(&mut o, 6, 1, "a\nb", "");
        assert_eq!(o, "\"a\\nb\"");
    }

    #[test]
    fn token_trees_go_through_flat_and_back() {
        let src = "/// doc\n#[a(b = \"c\")] pub fn r#type<'a>(x: &'a u8) -> u32 { x as u32 + 1u32 } a::b => [1, 2.5]";
        for v in [4, 6] {
            let ft = parse_json(&flat(TokenStream::from_str(src).unwrap(), v)).unwrap();
            let back = unflat(&ft, v).unwrap();
            assert_eq!(
                TokenStream::from_str(&back).unwrap().to_string(),
                TokenStream::from_str(src).unwrap().to_string(),
                "version {v}: {back}"
            );
            assert!(!back.contains('\n'));
        }
    }

    #[test]
    fn json_reads_what_it_writes() {
        let j = parse_json(&format!("{{\"a\":[1,-2.5e1,true,null],\"b\":{}}}", js("q\"\\\n\u{1}é😀"))).unwrap();
        assert_eq!(j.get("a").and_then(|a| a.at(1)).and_then(J::num), Some(-25.0));
        assert_eq!(j.get("b").and_then(J::str), Some("q\"\\\n\u{1}é😀"));
        assert_eq!(parse_json("\"\\ud83d\\ude00\"").unwrap(), J::Str("😀".into()));
    }

    #[test]
    fn uses_are_found_by_name() {
        let libs = vec![Lib {
            name: "pm".into(),
            dylib: PathBuf::new(),
            macros: vec![("route".into(), Kind::Attr), ("Error".into(), Kind::Derive), ("html".into(), Kind::Bang)],
        }];
        let src = "use pm::route as get;\nuse derive_more::Debug;\n";
        let b = src.as_bytes();
        let toks = crate::lexer::tokenize(b);
        let s = Scope::new(&libs, b, &toks);
        assert_eq!(s.resolve(&["get"], Kind::Attr).map(|x| x.1), Some("route".into()));
        assert_eq!(s.resolve(&["pm", "route"], Kind::Attr).map(|x| x.1), Some("route".into()));
        assert!(s.resolve(&["std", "route"], Kind::Attr).is_none());
        assert!(s.resolve(&["inline"], Kind::Attr).is_none());
        assert_eq!(s.resolve(&["Error"], Kind::Derive).map(|x| x.1), Some("Error".into()));
        assert!(s.resolve(&["Error"], Kind::Attr).is_none());
        assert_eq!(s.resolve(&["html"], Kind::Bang).map(|x| x.1), Some("html".into()));
        // (two crates with one macro's name: the one the `use` names)
        let two = vec![
            Lib { name: "a".into(), dylib: PathBuf::from("a"), macros: vec![("make".into(), Kind::Bang)] },
            Lib { name: "b".into(), dylib: PathBuf::from("b"), macros: vec![("make".into(), Kind::Bang)] },
        ];
        let src = "use b::make;\n";
        let toks = crate::lexer::tokenize(src.as_bytes());
        let s2 = Scope::new(&two, src.as_bytes(), &toks);
        assert_eq!(s2.resolve(&["make"], Kind::Bang).map(|x| x.0.name.clone()), Some("b".into()));
        assert_eq!(s2.resolve(&["a", "make"], Kind::Bang).map(|x| x.0.name.clone()), Some("a".into()));
    }

    #[test]
    fn items_end_where_rustc_ends_them() {
        let end = |src: &str| -> Option<String> {
            let b = src.as_bytes();
            let toks = crate::lexer::tokenize(b);
            let (close, depth) = crate::macros::brackets_of(b, &toks);
            item_end(b, &toks, &close, &depth, 0).map(|e| src[..toks[e].2].to_string())
        };
        assert_eq!(end("pub fn f() -> [u8; 2] { [0; 2] } struct S;").as_deref(), Some("pub fn f() -> [u8; 2] { [0; 2] }"));
        assert_eq!(end("const X: u8 = { 1 }; fn g() {}").as_deref(), Some("const X: u8 = { 1 };"));
        assert_eq!(end("pub(crate) struct S(u8); fn g() {}").as_deref(), Some("pub(crate) struct S(u8);"));
        assert_eq!(end("extern \"C\" { type T; } fn g() {}").as_deref(), Some("extern \"C\" { type T; }"));
        assert_eq!(end("unsafe impl<T> X for Y<T> where T: Fn() -> u8 {} x").as_deref(), Some("unsafe impl<T> X for Y<T> where T: Fn() -> u8 {}"));
        assert_eq!(end("fn f() -> A<{ 1 }> where T: Fn() -> u8 { loop {} } x").as_deref(), Some("fn f() -> A<{ 1 }> where T: Fn() -> u8 { loop {} }"));
        assert_eq!(end("let x = 1;"), None);
        assert_eq!(end("pub x: u8 }"), None);
    }
}
