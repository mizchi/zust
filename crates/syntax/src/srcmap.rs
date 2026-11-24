//! 複数ファイルのクレート: `mod x;` を `x.rs` (か `x/mod.rs`) の中身で置き換えて 1 つのソースにし、
//! 結合したソースの行 → (ファイル, ファイルの中の行) の対応 (SrcMap) を作る。
//!
//! 行番号をずらさないため、`mod x;` は同じ長さの空白にして、中身はファイルの最後に `pub mod x { .. }` として足す
//! (Rust ではアイテムの順は意味を持たない)。コンパイルエラーの `行:桁:` と panic の場所はこの対応で元のファイルに戻す。

use std::path::{Path, PathBuf};

/// 結合したソースの行の区間: (開始行 (1 始まり), ファイルの番号, そのファイルでの開始行)
#[derive(Clone, Debug, Default)]
pub struct SrcMap {
    pub segs: Vec<(u32, u32, u32)>,
    /// 表示するファイルの名前 (`src/main.rs`, `src/foo/bar.rs` など)
    pub files: Vec<String>,
    /// 読んだファイル
    pub inputs: Vec<PathBuf>,
    /// `mod x;` で探した場所 (`x.rs` と `x/mod.rs`; 無いものも): ここに現れる・消えるファイルは読むものを変える
    pub probes: Vec<PathBuf>,
    /// 読んだファイルの外のもの (include_str!, include_bytes!) に依るか: ファイルのスタンプだけでは新しさが言えない
    pub volatile: bool,
    /// モジュールの名前を変えて桁がずれた所: (行, 桁, ずれ) (lexer::rename_module_collisions_fix)
    pub colfix: Vec<(u32, u32, i32)>,
    /// The root file's `extern crate x;` items, which the configuring blanks: (byte offset, text)
    /// (cfg::strip_noting). Only read when `keep_externs` is set before loading (load_crate_keeping_externs)
    pub externs: Vec<(usize, String)>,
    /// Whether the loading keeps `externs`
    pub keep_externs: bool,
}

impl SrcMap {
    /// 1 ファイルだけ (区間 1 つ)
    pub fn single(name: &str) -> SrcMap {
        SrcMap { segs: vec![(1, 0, 1)], files: vec![name.to_string()], inputs: vec![], probes: vec![], volatile: false, colfix: vec![], externs: vec![], keep_externs: false }
    }

    /// 結合したソースの行 → (ファイルの名前, ファイルの中の行)
    pub fn locate(&self, line: u32) -> (&str, u32) {
        let mut best = (0u32, 1u32, 1u32);
        for &(s, f, first) in &self.segs {
            if s <= line {
                best = (f, s, first);
            }
        }
        let name = self.files.get(best.0 as usize).map(|s| s.as_str()).unwrap_or("src/main.rs");
        (name, line - best.1 + best.2)
    }

    /// 結合したソースの (行, 桁) の桁を、名前を変える前の桁にする
    pub fn fix_col(&self, line: u32, col: u32) -> u32 {
        let mut d = 0;
        for &(l, c, cum) in &self.colfix {
            if l == line && c <= col {
                d = cum;
            }
        }
        (col as i64 - d as i64).max(0) as u32
    }

    /// relocate_err, and a second place `(at L:C)` in the message: `(at L:C)` in the same file, else `(at file:L:C)`
    pub fn relocate_err(&self, e: &str) -> String {
        let head = self.relocate_head(e);
        let Some(k) = head.find("(at ") else { return head };
        let rest = &head[k + 4..];
        let b = rest.as_bytes();
        let mut i = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if i == 0 || i >= b.len() || b[i] != b':' || j == i + 1 || j >= b.len() || b[j] != b')' {
            return head;
        }
        let line: u32 = rest[..i].parse().unwrap_or(0);
        let col: u32 = rest[i + 1..j].parse().unwrap_or(0);
        let (f, l) = self.locate(line);
        let c = self.fix_col(line, col);
        let main_file = head.split(':').next().unwrap_or("");
        let place = if f == main_file { format!("{l}:{c}") } else { format!("{f}:{l}:{c}") };
        format!("{}(at {place}){}", &head[..k], &rest[j + 1..])
    }

    /// エラーの先頭の `行:桁:` (か `行:`) を `ファイル:行:桁:` にする
    fn relocate_head(&self, e: &str) -> String {
        let b = e.as_bytes();
        let mut i = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == 0 || i >= b.len() || b[i] != b':' {
            return e.to_string();
        }
        let line: u32 = e[..i].parse().unwrap_or(0);
        let (f, l) = self.locate(line);
        // (桁)
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > i + 1 && j < b.len() && b[j] == b':' {
            let col: u32 = e[i + 1..j].parse().unwrap_or(0);
            return format!("{f}:{l}:{}{}", self.fix_col(line, col), &e[j..]);
        }
        format!("{f}:{l}{}", &e[i..])
    }
}

fn line_count(s: &str) -> u32 {
    s.as_bytes().iter().filter(|&&c| c == b'\n').count() as u32
}

/// クレートの根のファイル (main.rs / lib.rs) を読み、`mod x;` を展開する。display は根の表示名 (`src/main.rs` など)
pub fn load_crate(root: &Path, display: &str, cfg: &crate::cfg::CfgSet) -> Result<(String, SrcMap), String> {
    load_crate_in(root, display, cfg, SrcMap::default())
}

/// [`load_crate`], keeping the root file's `extern crate` items that the configuring blanks (SrcMap::externs): for
/// `zustc check`, which reads the names they define
pub fn load_crate_keeping_externs(root: &Path, display: &str, cfg: &crate::cfg::CfgSet) -> Result<(String, SrcMap), String> {
    load_crate_in(root, display, cfg, SrcMap { keep_externs: true, ..SrcMap::default() })
}

fn load_crate_in(root: &Path, display: &str, cfg: &crate::cfg::CfgSet, mut map: SrcMap) -> Result<(String, SrcMap), String> {
    let dir = root.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let disp_dir = match display.rfind('/') {
        Some(i) => display[..i].to_string(),
        None => String::new(),
    };
    let mut open = vec![];
    let text = load_file(root, display, dir.as_path(), &disp_dir, 1, &mut map, cfg, &mut open)?;
    Ok((text, map))
}

unsafe extern "C" {
    #[link_name = "realpath"]
    fn c_realpath(path: *const u8, resolved: *mut u8) -> *mut u8;
    #[link_name = "free"]
    fn c_free(p: *mut u8);
}

/// Which file a path names, to find circular modules: the path with symbolic links resolved (libc realpath: this
/// file is also compiled by zustc, whose std has no canonicalize). `link/../a.rs` is a.rs beside where link
/// points, not beside link. When realpath fails, the path with `.` and `x/..` taken out as text
fn file_key(p: &Path) -> String {
    let c = format!("{}\0", p.display());
    // SAFETY: c ends with NUL; with a null buffer realpath returns a malloc'd NUL-terminated string (or null)
    let r = unsafe { c_realpath(c.as_bytes().as_ptr(), std::ptr::null_mut()) };
    if r.is_null() {
        return lexical(p);
    }
    let mut out: Vec<u8> = vec![];
    let mut i = 0;
    // SAFETY: r is NUL-terminated
    while unsafe { *r.add(i) } != 0 {
        out.push(unsafe { *r.add(i) });
        i += 1;
    }
    unsafe { c_free(r) };
    String::from_utf8_lossy(&out).into_owned()
}

/// The path with `.` and `x/..` taken out (as text: two spellings of one file compare equal)
fn lexical(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    let abs = s.starts_with('/');
    let mut parts: Vec<&str> = vec![];
    for c in s.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if c == ".." && parts.last().is_some_and(|l| *l != "..") {
            parts.pop();
            continue;
        }
        parts.push(c);
    }
    let j = parts.join("/");
    if abs { format!("/{j}") } else { j }
}

/// ファイル path を読んで展開する。mod_dir は子のモジュールのファイルを探すディレクトリ。start は結合したソースでの開始行。
/// open は読んでいる途中のファイル (根から): 同じファイルがまた出てきたら循環 (rustc と同じく "circular modules")
#[allow(clippy::too_many_arguments)]
fn load_file(path: &Path, display: &str, mod_dir: &Path, disp_dir: &str, start: u32, map: &mut SrcMap, cfg: &crate::cfg::CfgSet, open: &mut Vec<(String, String)>) -> Result<String, String> {
    let key = file_key(path);
    if let Some(i) = open.iter().position(|o| o.0 == key) {
        let chain: Vec<&str> = open[i..].iter().chain(std::iter::once(&open[i])).map(|o| o.1.as_str()).collect();
        return Err(format!("{}: circular modules: {}", open[open.len() - 1].1, chain.join(" -> ")));
    }
    // (spellings the text does not catch, such as symbolic links, still stop)
    if open.len() >= 256 {
        return Err(format!("{display}: modules nested more than 256 deep"));
    }
    open.push((key, display.to_string()));
    let r = load_file_in(path, display, mod_dir, disp_dir, start, map, cfg, open);
    open.pop();
    r
}

/// src with its `env!` / `option_env!` expanded (not inlined: load_file_in's loop over every file stays as it was)
#[inline(never)]
fn env_values(src: String, cfg: &crate::cfg::CfgSet) -> String {
    crate::macros::expand_env(&src, cfg).unwrap_or(src)
}

#[allow(clippy::too_many_arguments)]
fn load_file_in(path: &Path, display: &str, mod_dir: &Path, disp_dir: &str, start: u32, map: &mut SrcMap, cfg: &crate::cfg::CfgSet, open: &mut Vec<(String, String)>) -> Result<String, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // (edition 2021's reserved prefixes, before any macro takes the tokens as its input: `m!(foo#bar)`)
    if cfg.edition >= 2021 {
        if let Some(e) = crate::lexer::unknown_prefix(&src) {
            return Err(format!("{display}:{e}"));
        }
    }
    // (cfg を先に評価する: 偽の `#[cfg(..)] mod x;` のファイルは読まない)
    // (and whether it may call `env!` / `option_env!`, seen by the same tokens)
    let mut env = None;
    let mut externs = vec![];
    let src = match crate::cfg::strip_noting(&src, cfg, &mut env, &mut externs) {
        Some(t) => {
            // (the root file's `extern crate` items, for a caller that asked)
            if map.inputs.is_empty() && map.keep_externs {
                map.externs = externs.iter().map(|&(a, b)| (a, src[a..b].to_string())).collect();
            }
            t
        }
        None => src,
    };
    // (the file as written, its cfgs evaluated, before any macro: a `mod x;` not in it came from a macro's body
    // (written_mod). Kept as it is; what `env!` / `include!` change is a text of its own, so no file is copied)
    let raw = src;
    map.inputs.push(path.to_path_buf());
    // (the configured root, while its `extern crate` items are kept: the expansions below may move what follows)
    let configured = if map.inputs.len() == 1 && !map.externs.is_empty() { Some(raw.clone()) } else { None };
    // (`env!` / `option_env!` read the environment, which file stamps do not cover. A workspace's no-op build reads
    // every file: one more look at the source only when the cfg pass did not read its tokens)
    let mut changed: Option<String> = None;
    if env.unwrap_or_else(|| crate::macros::may_use_env(&raw)) {
        map.volatile = true;
        changed = crate::macros::expand_env(&raw, cfg);
    }
    match crate::macros::expand_includes(changed.as_deref().unwrap_or(&raw), path.parent().unwrap_or(Path::new("."))) {
        Ok(Some(s)) => {
            map.volatile = true;
            // (and the `env!` of what `include!` put in: before it, `env!` gave the include's path)
            changed = Some(if crate::macros::may_use_env(&s) { env_values(s, cfg) } else { s });
        }
        Ok(None) => {}
        Err(e) => return Err(format!("{display}: {e}")),
    }
    let src: &str = changed.as_deref().unwrap_or(&raw);
    // (the root's `extern crate` items keep their places before the first byte an expansion changed)
    if let Some(c) = configured {
        let same = c.bytes().zip(src.bytes()).take_while(|(a, b)| a == b).count();
        map.externs.retain(|(p, x)| p + x.len() <= same);
    }
    let fi = map.files.len() as u32;
    map.files.push(display.to_string());
    map.segs.push((start, fi, 1));
    let (blanked, kids) = blank_mod_decls(&src).map_err(|e| format!("{display}: {e}"))?;
    let end_line = 1 + line_count(&src);
    if kids.is_empty() {
        return hoist_macro_mods(blanked, path, mod_dir, disp_dir, start, map, cfg, open, fi, end_line);
    }
    // (a `mod x;` inside inline modules: its file goes in that place, `pub mod x { .. }` on lines of its own; the
    // rest of the line continues on the line of the closing brace)
    let mut text = String::new();
    let mut last = 0;
    let mut top = vec![];
    for (name, path_attr, nested) in kids {
        let Some((pos, parts)) = nested else {
            top.push((name, path_attr));
            continue;
        };
        text.push_str(&blanked[last..pos]);
        last = pos;
        let file_line = 1 + line_count(&blanked[..pos]);
        let mut dir = mod_dir.to_path_buf();
        let mut ddisp = disp_dir.to_string();
        for p in &parts {
            let d = match p {
                Ok(n) | Err(n) => n,
            };
            dir = dir.join(d);
            ddisp = join_disp(&ddisp, d);
        }
        let (p, disp, sub, sub_disp) = match path_attr {
            // (`#[macro_use] #[path = "x.rs"] mod m;` outside inline modules: beside this file, as a top-level one)
            Some(pa) if parts.is_empty() => {
                let p = path.parent().unwrap_or(Path::new(".")).join(&pa);
                map.probes.push(p.clone());
                let sub = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
                (p, join_disp(disp_dir, &pa), sub, disp_dir.to_string())
            }
            Some(pa) => {
                let p = dir.join(&pa);
                let sub = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
                (p, join_disp(&ddisp, &pa), sub, ddisp.clone())
            }
            None => {
                let a = dir.join(format!("{name}.rs"));
                let b = dir.join(&name).join("mod.rs");
                map.probes.push(a.clone());
                map.probes.push(b.clone());
                // (a `mod x;` a macro body declares (serde_core's `crate_root!` in crate_root.rs, called in lib.rs): rustc
                // looks beside the file that calls it; beside this file is that for a macro called from its parent)
                let here = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                let mut hdir = here.clone();
                for p in &parts {
                    hdir = hdir.join(match p {
                        Ok(n) | Err(n) => n,
                    });
                }
                let (c, d) = (hdir.join(format!("{name}.rs")), hdir.join(&name).join("mod.rs"));
                if a.exists() {
                    (a, join_disp(&ddisp, &format!("{name}.rs")), dir.join(&name), join_disp(&ddisp, &name))
                } else if b.exists() {
                    (b, join_disp(&ddisp, &format!("{name}/mod.rs")), dir.join(&name), join_disp(&ddisp, &name))
                } else if hdir != dir && !written_mod(&raw, &name) && (c.exists() || d.exists()) {
                    map.probes.push(c.clone());
                    map.probes.push(d.clone());
                    let pdisp = match display.rsplit_once('/') {
                        Some((dd, _)) => dd.to_string(),
                        None => String::new(),
                    };
                    if c.exists() {
                        (c, join_disp(&pdisp, &format!("{name}.rs")), hdir.join(&name), join_disp(&pdisp, &name))
                    } else {
                        (d, join_disp(&pdisp, &format!("{name}/mod.rs")), hdir.join(&name), join_disp(&pdisp, &name))
                    }
                } else {
                    return Err(format!("{display}: file not found for module `{name}`"));
                }
            }
        };
        text.push_str(&format!("\npub mod {name} {{\n"));
        let at = start + line_count(&text);
        let body = load_file(p.as_path(), &disp, sub.as_path(), &sub_disp, at, map, cfg, open)?;
        text.push_str(&body);
        text.push_str("\n}");
        map.segs.push((start + line_count(&text), fi, file_line));
    }
    text.push_str(&blanked[last..]);
    if top.is_empty() {
        return hoist_macro_mods(text, path, mod_dir, disp_dir, start, map, cfg, open, fi, end_line);
    }
    text.push('\n');
    for (name, path_attr) in top {
        // (a `#` in front: the declaration had `#[macro_use]`, which the appended module keeps)
        let (name, mu) = match name.strip_prefix('#') {
            Some(n) => (n.to_string(), "#[macro_use] "),
            None => (name, ""),
        };
        // (`#[path = "x.rs"] mod m;`: that file, beside this one; its own modules are beside it too)
        if let Some(pa) = path_attr {
            let here = path.parent().unwrap_or(Path::new("."));
            let p = here.join(pa.as_str());
            map.probes.push(p.clone());
            let disp = join_disp(disp_dir, &pa);
            let sub = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            text.push_str(&format!("{mu}pub mod {name} {{\n"));
            let at = start + line_count(&text);
            let body = load_file(p.as_path(), &disp, sub.as_path(), disp_dir, at, map, cfg, open)?;
            text.push_str(&body);
            text.push_str("\n}\n");
            map.segs.push((start + line_count(&text) - 1, fi, 1 + line_count(&src)));
            continue;
        }
        let a = mod_dir.join(format!("{name}.rs").as_str());
        let b = mod_dir.join(name.as_str()).join("mod.rs");
        map.probes.push(a.clone());
        map.probes.push(b.clone());
        let (p, disp, sub, sub_disp) = if a.exists() {
            (a, join_disp(disp_dir, &format!("{name}.rs")), mod_dir.join(name.as_str()), join_disp(disp_dir, &name))
        } else if b.exists() {
            (b, join_disp(disp_dir, &format!("{name}/mod.rs")), mod_dir.join(name.as_str()), join_disp(disp_dir, &name))
        } else {
            return Err(format!("{display}: file not found for module `{name}`"));
        };
        text.push_str(&format!("{mu}pub mod {name} {{\n"));
        let at = start + line_count(&text);
        let body = load_file(p.as_path(), &disp, sub.as_path(), &sub_disp, at, map, cfg, open)?;
        text.push_str(&body);
        text.push_str("\n}\n");
        // (`}` の行は、元のファイルの最後の行の続きにしておく)
        map.segs.push((start + line_count(&text) - 1, fi, 1 + line_count(&src)));
    }
    hoist_macro_mods(text, path, mod_dir, disp_dir, start, map, cfg, open, fi, end_line)
}

/// `mod x;` declared in the body of a `macro_rules!` that this file invokes at its own module level (outside inline
/// modules): rustc reads x.rs where the macro is invoked, as a module of the invoking file (libc 0.2's `prelude!`,
/// defined in src/macros.rs and invoked in src/lib.rs, declares `mod types;`: src/types.rs). The file is read here,
/// appended as `pub mod x { .. }`, and the declaration in the macro's body is blanked so the expansion does not
/// declare the module a second time. Invocations inside inline modules are left as they were.
#[allow(clippy::too_many_arguments)]
fn hoist_macro_mods(text: String, path: &Path, mod_dir: &Path, disp_dir: &str, start: u32, map: &mut SrcMap, cfg: &crate::cfg::CfgSet, open: &mut Vec<(String, String)>, fi: u32, end_line: u32) -> Result<String, String> {
    if !text.contains("macro_rules") || !text.contains("mod") {
        return Ok(text);
    }
    let b = text.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && b[toks[k].1] == c;
    let opens = |k: usize| is_sym(k, b'{') || is_sym(k, b'(') || is_sym(k, b'[');
    let closes = |k: usize| is_sym(k, b'}') || is_sym(k, b')') || is_sym(k, b']');
    // 1. the macros' bodies (token ranges) and the out-of-line modules they declare: (macro, module, #[path], bytes)
    let mut bodies: Vec<(usize, usize)> = vec![];
    let mut decls: Vec<(String, String, Option<String>, usize, usize)> = vec![];
    // (the macros each macro's body calls: `outer!()` that calls `inner!()`, whose body declares the module)
    let mut calls: Vec<(String, Vec<String>)> = vec![];
    let mut k = 0;
    while k + 3 < toks.len() {
        if toks[k].0 == 0 && tk(k) == b"macro_rules" && is_sym(k + 1, b'!') && toks[k + 2].0 == 0 && opens(k + 3) {
            let name = String::from_utf8_lossy(tk(k + 2)).into_owned();
            let (mut d, mut j) = (0i32, k + 3);
            while j < toks.len() {
                if opens(j) {
                    d += 1;
                } else if closes(j) {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                j += 1;
            }
            bodies.push((k + 3, j));
            let mut called = vec![];
            let mut m = k + 4;
            while m + 2 < j {
                if toks[m].0 == 0 && tk(m) == b"mod" && toks[m + 1].0 == 0 && is_sym(m + 2, b';') {
                    // (with its visibility, `pub` / `pub(crate)`, and its attributes, as blank_mod_decls takes them)
                    let mut st = m;
                    if st >= 1 && is_sym(st - 1, b')') {
                        let mut q = st - 1;
                        while q > 0 && !is_sym(q, b'(') {
                            q -= 1;
                        }
                        if q >= 1 && toks[q - 1].0 == 0 && tk(q - 1) == b"pub" {
                            st = q - 1;
                        }
                    } else if st >= 1 && toks[st - 1].0 == 0 && tk(st - 1) == b"pub" {
                        st -= 1;
                    }
                    let (b0, _, pa) = attrs_before(&text, b, &toks, st);
                    decls.push((name.clone(), String::from_utf8_lossy(tk(m + 1)).into_owned(), pa, toks[b0].1, toks[m + 2].2));
                    m += 3;
                    continue;
                }
                if toks[m].0 == 0 && is_sym(m + 1, b'!') && tk(m) != b"macro_rules" {
                    called.push(String::from_utf8_lossy(tk(m)).into_owned());
                }
                m += 1;
            }
            calls.push((name.clone(), called));
            k = j + 1;
            continue;
        }
        k += 1;
    }
    if decls.is_empty() {
        return Ok(text);
    }
    // 2. which of those macros this file invokes at its own module level: outside fn bodies, impls and inline
    //  modules, but also in another macro's arguments (`cfg_if! { if #[cfg(..)] { prelude!(); } }`). A call in a fn
    //  body declares the module in that block; zustc leaves it as it was. Then the macros they call, and so on
    let mut invoked: Vec<String> = vec![];
    // (each open bracket: whether it is a macro call's arguments, or inside them)
    let mut stack: Vec<bool> = vec![];
    let mut k = 0;
    while k < toks.len() {
        if let Some(&(_, e)) = bodies.iter().find(|&&(s, _)| s == k) {
            k = e + 1;
            continue;
        }
        if opens(k) {
            stack.push((k >= 1 && is_sym(k - 1, b'!')) || stack.last() == Some(&true));
        } else if closes(k) {
            stack.pop();
        } else if stack.iter().all(|&m| m) && toks[k].0 == 0 && k + 1 < toks.len() && is_sym(k + 1, b'!') && !(k >= 1 && is_sym(k - 1, b':')) {
            let n = String::from_utf8_lossy(tk(k)).into_owned();
            if calls.iter().any(|c| c.0 == n) && !invoked.contains(&n) {
                invoked.push(n);
            }
        }
        k += 1;
    }
    let mut i = 0;
    while i < invoked.len() {
        let more: Vec<String> = calls.iter().filter(|c| c.0 == invoked[i]).flat_map(|c| c.1.iter().cloned()).collect();
        for n in more {
            if calls.iter().any(|c| c.0 == n) && !invoked.contains(&n) {
                invoked.push(n);
            }
        }
        i += 1;
    }
    let mut out = b.to_vec();
    let mut mods: Vec<(String, Option<String>)> = vec![];
    for (mac, m, pa, s, e) in &decls {
        if !invoked.contains(mac) {
            continue;
        }
        for c in &mut out[*s..*e] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
        if !mods.iter().any(|x| &x.0 == m) {
            mods.push((m.clone(), pa.clone()));
        }
    }
    let mut text = String::from_utf8(out).map_err(|e| e.to_string())?;
    // 3. the modules' files, beside the invoking file, appended as at the top level
    for (name, pa) in mods {
        let (p, disp, sub, sub_disp) = if let Some(pa) = pa {
            let p = path.parent().unwrap_or(Path::new(".")).join(pa.as_str());
            map.probes.push(p.clone());
            let sub = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            (p, join_disp(disp_dir, &pa), sub, disp_dir.to_string())
        } else {
            let a = mod_dir.join(format!("{name}.rs"));
            let c = mod_dir.join(&name).join("mod.rs");
            map.probes.push(a.clone());
            map.probes.push(c.clone());
            if a.exists() {
                (a, join_disp(disp_dir, &format!("{name}.rs")), mod_dir.join(&name), join_disp(disp_dir, &name))
            } else if c.exists() {
                (c, join_disp(disp_dir, &format!("{name}/mod.rs")), mod_dir.join(&name), join_disp(disp_dir, &name))
            } else {
                return Err(format!("{}: file not found for module `{name}` (declared by a macro invoked here)", path.display()));
            }
        };
        text.push_str(&format!("\npub mod {name} {{\n"));
        let at = start + line_count(&text);
        let body = load_file(p.as_path(), &disp, sub.as_path(), &sub_disp, at, map, cfg, open)?;
        text.push_str(&body);
        text.push_str("\n}\n");
        map.segs.push((start + line_count(&text) - 1, fi, end_line));
    }
    Ok(text)
}

/// The file as written declares `mod name;` (an ordinary declaration: its file is only where its module path says)
fn written_mod(raw: &str, name: &str) -> bool {
    if !raw.contains(name) {
        return false;
    }
    // (on the tokens, so not in a comment or string; and outside the bodies of `macro_rules!`: a declaration in one
    // is the macro's, made where it is called)
    let b = raw.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let tk = |k: usize| &b[toks[k].1..toks[k].2];
    let sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && b[toks[k].1] == c;
    let mut i = 0;
    while i < toks.len() {
        if toks[i].0 == 0 && tk(i) == b"macro_rules" && sym(i + 1, b'!') && (sym(i + 3, b'{') || sym(i + 3, b'(') || sym(i + 3, b'[')) {
            let mut d = 0i32;
            let mut j = i + 3;
            while j < toks.len() {
                if sym(j, b'{') || sym(j, b'(') || sym(j, b'[') {
                    d += 1;
                } else if sym(j, b'}') || sym(j, b')') || sym(j, b']') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                j += 1;
            }
            i = j + 1;
            continue;
        }
        if toks[i].0 == 0 && tk(i) == b"mod" && i + 2 < toks.len() && toks[i + 1].0 == 0 && tk(i + 1) == name.as_bytes() && sym(i + 2, b';') {
            return true;
        }
        i += 1;
    }
    false
}

fn join_disp(dir: &str, rest: &str) -> String {
    if dir.is_empty() { rest.to_string() } else { format!("{dir}/{rest}") }
}

/// トップレベルの `[pub[(..)]] mod x;` を空白にし、(名前, #[cfg(test)] か) の列を返す。`#[cfg(test)]` のものは読まない
/// A `mod x;` inside inline modules (`mod a { #[path = "d"] mod b { mod x; } }`): where it is in the text and the
/// directories of the modules around it, outermost first (Ok: the module's name, Err: its `#[path]`)
type Nested = Option<(usize, Vec<Result<String, String>>)>;

/// The `#[path = ".."]` among the attributes that end just before token b, and the first token of those attributes
fn attrs_before(src: &str, s: &[u8], toks: &[(u8, usize, usize)], mut b: usize) -> (usize, bool, Option<String>) {
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let mut cfg_test = false;
    let mut path_attr: Option<String> = None;
    while b >= 1 && is_sym(b - 1, b']') {
        let mut j = b - 1;
        while j > 0 && !is_sym(j, b'[') {
            j -= 1;
        }
        if j >= 1 && is_sym(j - 1, b'#') {
            let attr = &src[toks[j].1..toks[b - 1].2];
            if attr.contains("cfg") && attr.contains("test") {
                cfg_test = true;
            }
            // (`#[path = "bool.rs"]`: the file to read, relative to this file's directory)
            let a = attr.trim_start_matches('[').trim();
            if let Some(r) = a.strip_prefix("path") {
                if let (Some(i), Some(j)) = (r.find('"'), r.rfind('"')) {
                    if j > i {
                        path_attr = Some(r[i + 1..j].to_string());
                    }
                }
            }
            b = j - 1;
        } else {
            break;
        }
    }
    (b, cfg_test, path_attr)
}

#[allow(clippy::type_complexity)]
#[inline(never)]
fn blank_mod_decls(src: &str) -> Result<(String, Vec<(String, Option<String>, Nested)>), String> {
    if !src.contains("mod") {
        return Ok((src.to_string(), vec![]));
    }
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let mut out = s.to_vec();
    let mut kids = vec![];
    let mut depth = 0i32;
    // (the inline modules open around here: (depth inside it, its directory part))
    let mut inline: Vec<(i32, Result<String, String>)> = vec![];
    let mut k = 0;
    while k < toks.len() {
        // (the body of `macro_rules! name { .. }` (or `( .. );` / `[ .. ];`) defined at the file's own level is tokens,
        //  not items: a `mod x;` there is declared where the macro is invoked (hoist_macro_mods). A macro inside an
        //  inline module is invoked there too, as far as zustc follows: its `mod x;` is read in that module, as before)
        if inline.is_empty() && toks[k].0 == 0 && tok(k) == b"macro_rules" && k + 3 < toks.len() && is_sym(k + 1, b'!') && toks[k + 2].0 == 0 {
            let open = k + 3;
            if is_sym(open, b'{') || is_sym(open, b'(') || is_sym(open, b'[') {
                let mut d = 0i32;
                let mut j = open;
                while j < toks.len() {
                    if is_sym(j, b'{') || is_sym(j, b'(') || is_sym(j, b'[') {
                        d += 1;
                    } else if is_sym(j, b'}') || is_sym(j, b')') || is_sym(j, b']') {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                k = j + 1;
                continue;
            }
        }
        if is_sym(k, b'{') {
            depth += 1;
        } else if is_sym(k, b'}') {
            if inline.last().is_some_and(|m| m.0 == depth) {
                inline.pop();
            }
            depth -= 1;
        } else if toks[k].0 == 0 && tok(k) == b"mod" && k + 2 < toks.len() && toks[k + 1].0 == 0 && is_sym(k + 2, b'{') {
            let (_, _, mut pa) = attrs_before(src, s, &toks, k);
            // (an inner `#![path = "d"]` right inside the braces)
            if let Some(r) = src[toks[k + 2].2..].trim_start().strip_prefix("#![") {
                if let Some(r) = r.trim_start().strip_prefix("path").map(str::trim_start).and_then(|r| r.strip_prefix('=')) {
                    if let Some(v) = r.trim_start().strip_prefix('"').and_then(|v| v.find('"').map(|e| &v[..e])) {
                        pa = Some(v.to_string());
                    }
                }
            }
            let name = String::from_utf8_lossy(tok(k + 1)).into_owned();
            inline.push((depth + 1, pa.ok_or(name)));
        } else if toks[k].0 == 0 && tok(k) == b"mod" && k + 2 < toks.len() && toks[k + 1].0 == 0 && is_sym(k + 2, b';') {
            // (`#[macro_use] mod x;`: the module keeps the attribute where it is appended (macros::expand_cfg takes
            //  its macros as in scope from the top))
            let (b_attr, _, _) = attrs_before(src, s, &toks, k);
            let macro_use = depth == 0 && src[toks[b_attr].1..toks[k].1].contains("macro_use");
            let nested = (depth != 0).then(|| (toks[k + 2].2, inline.iter().map(|m| m.1.clone()).collect::<Vec<_>>()));
            // (前の `pub` / `pub(crate)` と `#[..]` も消す)
            let mut b = k;
            if b >= 1 && is_sym(b - 1, b')') {
                let mut j = b - 1;
                while j > 0 && !is_sym(j, b'(') {
                    j -= 1;
                }
                if j >= 1 && toks[j - 1].0 == 0 && tok(j - 1) == b"pub" {
                    b = j - 1;
                }
            } else if b >= 1 && toks[b - 1].0 == 0 && tok(b - 1) == b"pub" {
                b -= 1;
            }
            let (b2, cfg_test, path_attr) = attrs_before(src, s, &toks, b);
            b = b2;
            // (`#[macro_use] mod m;`: m's macros are usable right after the declaration, so its file goes in at that
            // place, not after the file's end: serde_core's `#[macro_use] mod crate_root; crate_root!();`)
            let macro_use = src[toks[b].1..toks[k].1].contains("macro_use");
            let nested = nested.or_else(|| macro_use.then(|| (toks[k + 2].2, vec![])));
            for c in &mut out[toks[b].1..toks[k + 2].2] {
                if *c != b'\n' {
                    *c = b' ';
                }
            }
            if !cfg_test {
                let name = String::from_utf8_lossy(tok(k + 1)).into_owned();
                kids.push((if macro_use { format!("#{name}") } else { name }, path_attr, nested));
            }
            k += 3;
            continue;
        }
        k += 1;
    }
    Ok((String::from_utf8(out).map_err(|e| e.to_string())?, kids))
}

#[cfg(test)]
mod circular_tests {
    /// A module that includes itself, or two that include each other, is an error, not an endless recursion
    /// (tests/ui parser/circular_modules_main.rs, circular-module-with-doc-comment-issue-97589)
    #[test]
    fn circular_modules() {
        let dir = std::env::temp_dir().join(format!("srcmap_circular_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "#[path = \"b.rs\"]\nmod b;\nfn main() {}\n").unwrap();
        std::fs::write(dir.join("b.rs"), "#[path = \"./a.rs\"]\nmod a;\n").unwrap();
        std::fs::write(dir.join("r.rs"), "pub mod s;\n").unwrap();
        std::fs::write(dir.join("s.rs"), "//! doc\n#[path = \"s.rs\"]\nmod s;\n").unwrap();
        std::fs::write(dir.join("ok.rs"), "#[path = \"b2.rs\"]\nmod b2;\n#[path = \"b2.rs\"]\nmod b3;\nfn main() {}\n").unwrap();
        std::fs::write(dir.join("b2.rs"), "pub fn f() {}\n").unwrap();
        let cfg = crate::cfg::CfgSet::default();
        let e = super::load_crate(&dir.join("a.rs"), "src/main.rs", &cfg).unwrap_err();
        assert!(e.ends_with("circular modules: src/main.rs -> src/b.rs -> src/main.rs"), "{e}");
        let e = super::load_crate(&dir.join("r.rs"), "src/lib.rs", &cfg).unwrap_err();
        assert!(e.ends_with("circular modules: src/s.rs -> src/s.rs"), "{e}");
        // (the same file twice, side by side, is not a cycle)
        assert!(super::load_crate(&dir.join("ok.rs"), "src/main.rs", &cfg).is_ok());
        // (p/a.rs names `link/../a.rs` with p/link -> q/sub: that is q/a.rs, another file, not a cycle)
        #[cfg(unix)]
        {
            for d in ["p", "q/sub"] {
                std::fs::create_dir_all(dir.join(d)).unwrap();
            }
            std::os::unix::fs::symlink(dir.join("q/sub"), dir.join("p/link")).unwrap();
            std::fs::write(dir.join("p/a.rs"), "#[path = \"link/../a.rs\"]\nmod child;\nfn main() {}\n").unwrap();
            std::fs::write(dir.join("q/a.rs"), "pub fn f() {}\n").unwrap();
            let r = super::load_crate(&dir.join("p/a.rs"), "src/main.rs", &cfg);
            assert!(r.is_ok(), "{:?}", r.err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `mod x;` in the body of a `macro_rules!` is tokens, not a declaration: it is found where the macro is
    /// invoked, beside the invoking file (libc 0.2's `prelude!` in src/macros.rs declares `mod types;`, invoked in
    /// src/lib.rs, which means src/types.rs)
    #[test]
    fn mod_decl_inside_macro_rules() {
        let dir = std::env::temp_dir().join(format!("srcmap_macro_mod_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.rs"), "#[macro_use]\nmod macros;\nprelude!();\npub fn f() -> types::T { 1 }\n").unwrap();
        std::fs::write(
            dir.join("macros.rs"),
            "macro_rules! prelude {\n    () => {\n        mod types;\n    };\n}\nmacro_rules! other [ () => { mod nowhere; } ];\n",
        )
        .unwrap();
        std::fs::write(dir.join("types.rs"), "pub type T = u8;\n").unwrap();
        let cfg = crate::cfg::CfgSet::default();
        let r = super::load_crate(&dir.join("lib.rs"), "src/lib.rs", &cfg);
        assert!(r.is_ok(), "{:?}", r.err());
        let (src, _) = r.unwrap();
        // (types.rs is read, as a module of the invoking file, and the macro no longer declares it again)
        assert!(src.contains("pub type T = u8;"), "{src}");
        assert!(!src.contains("mod types;"), "{src}");
        // (`mod nowhere;` belongs to a macro nobody invokes: nothing is read for it)
        assert!(src.contains("mod nowhere;"), "{src}");
        // (the `#[macro_use]` module keeps its attribute where it is appended, so expansion sees its macros as in
        //  scope above it)
        assert!(src.contains("#[macro_use] pub mod macros {"), "{src}");
        let e = crate::macros::expand(&src);
        assert!(e.is_ok(), "{e:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tmp_crate(tag: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("srcmap_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (n, t) in files {
            let p = dir.join(n);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        }
        dir
    }

    /// The hoisted declaration goes with its visibility: `macro_rules! declare { () => { pub mod child; } }` left
    /// `pub` in the macro's body (`pub pub mod child` once expanded)
    #[test]
    fn hoisted_mod_takes_its_visibility() {
        let dir = tmp_crate("hoist_pub", &[("main.rs", "macro_rules! declare { () => { pub mod child; } }\ndeclare!();\nfn main() { child::f(); }\n"), ("child.rs", "pub fn f() {}\n")]);
        let (src, _) = super::load_crate(&dir.join("main.rs"), "src/main.rs", &crate::cfg::CfgSet::default()).unwrap();
        let norm = src.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(norm.contains("macro_rules! declare { () => { } }"), "{norm}");
        assert!(norm.contains("pub fn f() {}"), "{norm}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only calls at the module's level (also inside other macros' arguments, as `cfg_if! { .. }`) put a module in
    /// the file: a call in a fn body declares it in that block, which zustc leaves as it was
    #[test]
    fn no_hoisting_from_fn_bodies() {
        let dir = tmp_crate("hoist_fn", &[
            ("main.rs", "macro_rules! declare { () => { mod child; } }\nmacro_rules! wrap { ($($t:tt)*) => { $($t)* }; }\nwrap! { declare!(); }\nfn f() { declare!(); }\nfn main() {}\n"),
            ("child.rs", "pub const C: u8 = 1;\n"),
        ]);
        let (src, _) = super::load_crate(&dir.join("main.rs"), "src/main.rs", &crate::cfg::CfgSet::default()).unwrap();
        assert_eq!(src.matches("pub const C: u8 = 1;").count(), 1, "{src}");
        let dir2 = tmp_crate("hoist_fn_only", &[
            ("main.rs", "macro_rules! declare { () => { mod child; } }\nfn f() { declare!(); }\nfn main() {}\n"),
            ("child.rs", "pub const C: u8 = 1;\n"),
        ]);
        let (src2, _) = super::load_crate(&dir2.join("main.rs"), "src/main.rs", &crate::cfg::CfgSet::default()).unwrap();
        assert!(!src2.contains("pub const C: u8 = 1;"), "{src2}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    /// A module declared by a macro that another invoked macro calls is read too: `outer!()` calls `inner!()`,
    /// whose body declares `mod child;`
    #[test]
    fn hoisting_follows_nested_macro_calls() {
        let dir = tmp_crate("hoist_nested", &[
            ("main.rs", "macro_rules! inner { () => { mod child; } }\nmacro_rules! outer { () => { inner!(); } }\nouter!();\nfn main() {}\n"),
            ("child.rs", "pub const C: u8 = 1;\n"),
        ]);
        let (src, _) = super::load_crate(&dir.join("main.rs"), "src/main.rs", &crate::cfg::CfgSet::default()).unwrap();
        assert!(src.contains("pub const C: u8 = 1;"), "{src}");
        assert!(!src.contains("mod child;"), "{src}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A line below `#[macro_use] mod m;` maps back to its own line of the file: the module's body is put in place
    /// of the declaration, and the lines after it go on from the declaration's line (tests/ui
    /// macros/cross-file-errors.rs reports at the invocation, line 5)
    #[test]
    fn macro_use_module_keeps_lines() {
        let dir = std::env::temp_dir().join(format!("srcmap_macro_use_lines_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.rs"), "#[macro_use]\nmod underscore;\n\nfn main() {\n    underscore!();\n}\n").unwrap();
        std::fs::write(dir.join("underscore.rs"), "// a\n// b\n// c\nmacro_rules! underscore {\n    () => (\n        _\n    )\n}\n").unwrap();
        let cfg = crate::cfg::CfgSet::default();
        let (src, map) = super::load_crate(&dir.join("main.rs"), "src/main.rs", &cfg).unwrap();
        let line = 1 + src[..src.find("underscore!();").unwrap()].matches('\n').count() as u32;
        assert_eq!(map.locate(line), ("src/main.rs", 5), "{:?}\n{src}", map.segs);
        let def = 1 + src[..src.find("macro_rules!").unwrap()].matches('\n').count() as u32;
        assert_eq!(map.locate(def), ("src/underscore.rs", 4), "{:?}", map.segs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A macro defined and invoked inside an inline `#[path]` module declares its `mod x;` in that module, beside
    /// the module's directory (tests/ui modules/mod_dir_path.rs: `#[path = "auxiliary"] mod bar { macro_rules! m {
    /// () => { mod two_macros_2; } } m!(); }`)
    #[test]
    fn mod_decl_in_macro_inside_inline_module() {
        let dir = std::env::temp_dir().join(format!("srcmap_inline_macro_mod_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("auxiliary")).unwrap();
        std::fs::write(
            dir.join("main.rs"),
            "fn main() {\n    #[path = \"auxiliary\"]\n    mod bar {\n        macro_rules! m { () => { mod two; } }\n        m!();\n    }\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("auxiliary/two.rs"), "pub const TWO: u8 = 2;\n").unwrap();
        let cfg = crate::cfg::CfgSet::default();
        let r = super::load_crate(&dir.join("main.rs"), "src/main.rs", &cfg);
        assert!(r.is_ok(), "{:?}", r.err());
        assert!(r.unwrap().0.contains("pub const TWO: u8 = 2;"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `#[path]` module inside a fn body is relative to the directory of the file, as at the top level: a block
    /// is not a module and adds no directory (getrandom 0.4's backends/rdrand.rs has
    /// `fn is_rdrand_good() { #[path = "../utils/lazy_bool.rs"] mod lazy; .. }`, which is src/utils/lazy_bool.rs)
    #[test]
    fn path_mod_inside_fn_body() {
        let dir = std::env::temp_dir().join(format!("srcmap_fn_path_mod_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("backends")).unwrap();
        std::fs::create_dir_all(dir.join("utils")).unwrap();
        std::fs::write(dir.join("lib.rs"), "mod backends;\n").unwrap();
        std::fs::write(dir.join("backends.rs"), "mod rdrand;\n").unwrap();
        std::fs::write(
            dir.join("backends/rdrand.rs"),
            "fn good() -> bool {\n    #[path = \"../utils/lazy_bool.rs\"]\n    mod lazy;\n    lazy::YES\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("utils/lazy_bool.rs"), "pub const YES: bool = true;\n").unwrap();
        let cfg = crate::cfg::CfgSet::default();
        let r = super::load_crate(&dir.join("lib.rs"), "src/lib.rs", &cfg);
        assert!(r.is_ok(), "{:?}", r.err());
        assert!(r.unwrap().0.contains("pub const YES: bool = true;"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
