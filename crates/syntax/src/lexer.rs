//! 超軽量 lexer: トップレベルのアイテム分割と、アイテムごとの正規化ハッシュだけを行う。
//!
//! syn でファイル全体を parse すると 4MB/s 程度しか出ないので、no-op / 1 関数編集の
//! ビルドでは syn を使わず、この lexer だけで「どのアイテムが変わったか」を判定する。
//! 空白とコメントはハッシュに入らないので、整形やコメント編集では何も再コンパイルされない。

use crate::key::{Key, KeyHasher};

/// モジュールの名前の付け替えの表用の速いハッシュ (fxhash.rs と同じ方式。lexer は単独でもコンパイルされるので自前で持つ)
#[derive(Default, Clone, Copy)]
pub struct LexHasher {
    hash: u64,
}

impl std::hash::Hasher for LexHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.hash = (self.hash.rotate_left(5) ^ b as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    fn write_usize(&mut self, i: usize) {
        self.hash = (self.hash.rotate_left(5) ^ i as u64).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn finish(&self) -> u64 {
        self.hash
    }
}

type LMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<LexHasher>>;
type LSet<K> = std::collections::HashSet<K, std::hash::BuildHasherDefault<LexHasher>>;

fn name_hash(n: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in n {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub struct SrcItem {
    pub start: usize,
    pub end: usize,
    /// 1 始まりの開始行 (エラー位置の補正用)
    pub line: usize,
    /// 正規化トークン列のハッシュ。impl のメンバはヘッダのハッシュも含む
    pub hash: Key,
    /// アイテム内の識別子トークン (byte range)。参照関係の構文的な過大近似に使う
    pub idents: Vec<(u32, u32)>,
    pub kind: ItemKind,
}

/// `impl X { fn a() {} fn b() {} }` は 1 アイテムにせず、ヘッダ `impl X {` と
/// メンバ (a, b) に分ける。メソッドを 1 つ編集しても、そのメソッドだけが変わったことになる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Normal,
    /// `impl X {` まで
    ImplHeader,
    /// impl の中のアイテム。ヘッダの index を持つ
    ImplMember(u32),
    /// `trait X {` まで
    TraitHeader,
    /// trait の中のアイテム (デフォルト実装のない宣言も含む)
    TraitMember(u32),
    /// `#![no_std]` などのクレート属性
    InnerAttr,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}
fn is_ident_cont(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

type Cur = Option<(usize, usize, Vec<(u32, u32)>)>;

struct St<'a> {
    s: &'a [u8],
    cur: Cur,
    prev_ws: bool,
    prev_punct: bool,
    /// 正規化したトークン列。アイテム終端で 1 回だけハッシュする
    buf: Vec<u8>,
    /// 行番号はアイテム開始時だけ必要なので差分で数える
    line_cache: (usize, usize),
    /// 今のアイテムで、トップレベルの最初の識別子をもう見たか (`impl` 判定用)
    seen_ident: bool,
    /// アイテムの先頭の (pub などを除いた) 識別子 (最大 2 個、byte range)
    head: [(u32, u32); 2],
    n_head: usize,
}

impl St<'_> {
    fn line_at(&mut self, pos: usize) -> usize {
        let (p, l) = self.line_cache;
        let l = l + self.s[p..pos].iter().filter(|&&c| c == b'\n').count();
        self.line_cache = (pos, l);
        l
    }
    fn tok(&mut self, st: usize, en: usize, kind: u8) {
        if self.cur.is_none() {
            let ln = self.line_at(st);
            self.cur = Some((st, ln, vec![]));
        }
        let item = self.cur.as_mut().unwrap();

        // 記号同士が空白なしで連続しているか (`&&` と `& &` の区別) だけを入れる。
        // それ以外の空白はハッシュに影響しない
        // (`b"..".contains` は memchr になり、文字列の配置で速度が変わるので matches! にする)
        let op = kind == 5
            && matches!(
                self.s[st],
                b'&' | b'|'
                    | b'='
                    | b'<'
                    | b'>'
                    | b'!'
                    | b'-'
                    | b'+'
                    | b'*'
                    | b'/'
                    | b'%'
                    | b'^'
                    | b'.'
                    | b':'
                    | b'#'
                    | b'@'
                    | b'~'
                    | b'?'
                    | b'$'
            );
        let joint = op && self.prev_punct && !self.prev_ws;
        // (種類, 長さ, 本体) の列にする。長さを入れるので曖昧さがない
        self.buf.push(kind << 1 | joint as u8);
        self.buf
            .extend_from_slice(&((en - st) as u32).to_le_bytes());
        self.buf.extend_from_slice(&self.s[st..en]);
        self.prev_punct = op;
        if kind == 2 {
            item.2.push((st as u32, en as u32));
        }
        self.prev_ws = false;
    }
}

pub fn split(src: &str) -> Result<Vec<SrcItem>, String> {
    // (a closing delimiter that does not close the innermost open one is rustc's lexer's error, before any of the
    // parser's: read for only when the split fails)
    // (rustc 1.100 reports a closing delimiter with none open first, but a mismatched one only after a literal or a
    // block comment the file ends in: `(1; }` then `/* x` is E0758 at the comment, `"abc` E0765 at the string)
    split_items(src).map_err(|e| {
        let b = src.as_bytes();
        match mismatched_delim(b) {
            Some(m) if m.contains(": mismatched closing delimiter") => unterminated_literal(b).unwrap_or(m),
            Some(m) => m,
            None => e,
        }
    })
}

/// The line (from 1) of b's first merge diff marker, `<<<<<<<` at a line's start outside strings and comments
#[cold]
#[inline(never)]
pub fn diff_marker_line(b: &[u8]) -> Option<usize> {
    let at = tokenize(b).into_iter().find(|&(k, a, _)| k == 1 && b[a] == b'<' && (a == 0 || b[a - 1] == b'\n') && b[a..].starts_with(b"<<<<<<<"))?.1;
    Some(b[..at].iter().filter(|&&x| x == b'\n').count() + 1)
}

/// The lexer's error of b's delimiters as rustc 1.100 reports it, outside strings, chars and comments. A closing
/// delimiter that does not close the innermost open one takes that one away, and is read again for the next one
/// down when one of those it would close is open (`fn f() { bar(baz( }`), as rustc recovers; else it is skipped. A
/// closing delimiter with none open is "unexpected closing delimiter: `}`" there, which rustc reports in place of the
/// mismatches before it; without one, the first mismatch is "mismatched closing delimiter: `)`" at that delimiter
/// (where rustc labels it). None when every one matches (an unclosed one at the end is another error)
#[cold]
#[inline(never)]
fn mismatched_delim(b: &[u8]) -> Option<String> {
    let toks = tokenize(b);
    let at = |a: usize, what: &str| {
        let ls = b[..a].iter().rposition(|&x| x == b'\n').map_or(0, |p| p + 1);
        let line = b[..a].iter().filter(|&&x| x == b'\n').count() + 1;
        let col = std::str::from_utf8(&b[ls..a]).map_or(a - ls, |t| t.chars().count()) + 1;
        format!("{line}:{col}: {what} closing delimiter: `{}`", b[a] as char)
    };
    let mut open: Vec<u8> = vec![];
    let mut mismatch: Option<usize> = None;
    let mut k = 0;
    while k < toks.len() {
        let (kind, a, _) = toks[k];
        let want = match (kind, b[a]) {
            (1, c @ (b'(' | b'[' | b'{')) => {
                open.push(c);
                k += 1;
                continue;
            }
            (1, b')') => b'(',
            (1, b']') => b'[',
            (1, b'}') => b'{',
            _ => {
                k += 1;
                continue;
            }
        };
        match open.last() {
            None => return Some(at(a, "unexpected")),
            Some(&o) if o == want => {
                open.pop();
                k += 1;
            }
            Some(_) => {
                mismatch.get_or_insert(a);
                open.pop();
                if !open.contains(&want) {
                    k += 1;
                }
            }
        }
    }
    mismatch.map(|a| at(a, "mismatched"))
}

#[inline(never)]
fn split_items(src: &str) -> Result<Vec<SrcItem>, String> {
    let s = src.as_bytes();
    let n = s.len();
    let mut i = 0;
    let mut items: Vec<SrcItem> = vec![];
    let mut depth: i32 = 0;
    let mut st = St {
        s,
        cur: None,
        prev_ws: true,
        prev_punct: false,
        buf: Vec::with_capacity(4096),
        line_cache: (0, 1),
        seen_ident: false,
        head: [(0, 0); 2],
        n_head: 0,
    };
    // `use` / `const` / `static` / `type` は `}` ではなく `;` で終わる (`use a::{b, c};` / `const P: T = T { .. };`)
    let mut semi_only = false;
    // impl ブロックの中にいるときは、メンバの区切りを depth 1 で判定する
    let mut base: i32 = 0;
    let mut impl_header: Option<u32> = None;
    let mut impl_pending = false;
    let mut angle: i32 = 0;
    // ブロックが trait か impl か
    let mut is_trait = false;
    // `#![...]` (クレート属性) の途中か
    let mut inner_attr = false;
    macro_rules! tok {
        ($a:expr, $b:expr, $k:expr) => {
            st.tok($a, $b, $k)
        };
    }

    // (outer doc comments `///` / `/** */`: (start, end), looked at after the loop (E0585); only comments add to it)
    let mut docs: Vec<(usize, usize)> = Vec::new();
    while i < n {
        let c = s[i];
        // 空白
        if c.is_ascii_whitespace() {
            i += 1;
            st.prev_ws = true;
            continue;
        }
        // コメント
        if c == b'/' && i + 1 < n && s[i + 1] == b'/' {
            let st0 = i;
            while i < n && s[i] != b'\n' {
                i += 1;
            }
            if s[st0..].starts_with(b"///") && !s[st0..].starts_with(b"////") {
                docs.push((st0, i));
            }
            st.prev_ws = true;
            continue;
        }
        if c == b'/' && i + 1 < n && s[i + 1] == b'*' {
            let st0 = i;
            let mut d = 0;
            while i < n {
                if s[i] == b'/' && i + 1 < n && s[i + 1] == b'*' {
                    d += 1;
                    i += 2;
                } else if s[i] == b'*' && i + 1 < n && s[i + 1] == b'/' {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            // (the file ends in it: rustc's E0758 at its start, also after the last item, where no item is left
            // unsplit to report it. An unterminated literal before it is the error instead, as in rustc's lexer)
            if d != 0 {
                if let Some(e) = unterminated_literal(s) {
                    return Err(e);
                }
            }
            if s[st0..].starts_with(b"/**") && !s[st0..].starts_with(b"/***") && !s[st0..].starts_with(b"/**/") {
                docs.push((st0, i));
            }
            st.prev_ws = true;
            continue;
        }
        let start = i;
        // 文字列系リテラル (接頭辞 b / r / br / c / cr を含む)
        // (looked for only where one can start: most tokens start with another byte)
        if let Some(end) = string_lit_at(s, i) {
            i = end;
            tok!(start, i, 1u8);
            continue;
        }
        if is_ident_start(c) {
            if c == b'r' && i + 2 < n && s[i + 1] == b'#' && is_ident_start(s[i + 2]) {
                i += 2; // raw ident
            }
            while i < n && is_ident_cont(s[i]) {
                i += 1;
            }
            tok!(start, i, 2u8);
            if depth == base && st.n_head < 2 {
                let w = &s[start..i];
                if !matches!(w, b"pub" | b"crate" | b"super" | b"in") {
                    st.head[st.n_head] = (start as u32, i as u32);
                    st.n_head += 1;
                    let h0 = &s[st.head[0].0 as usize..st.head[0].1 as usize];
                    semi_only = match (h0, st.n_head) {
                        (b"use" | b"static" | b"type", _) => true,
                        (b"const", 1) => true,
                        (b"const", _) => {
                            let h1 = &s[st.head[1].0 as usize..st.head[1].1 as usize];
                            !matches!(h1, b"fn" | b"unsafe" | b"extern" | b"async")
                        }
                        _ => false,
                    };
                }
            }
            if depth == 0 && !st.seen_ident {
                let w = &s[start..i];
                // `unsafe impl` / `pub trait` に備えて unsafe と pub は読み飛ばす
                if w != b"unsafe" && w != b"pub" {
                    st.seen_ident = true;
                    impl_pending = w == b"impl" || w == b"trait";
                    is_trait = w == b"trait";
                }
            }
            continue;
        }
        if c.is_ascii_digit() {
            while i < n
                && (is_ident_cont(s[i]) || (s[i] == b'.' && i + 1 < n && s[i + 1].is_ascii_digit()))
            {
                i += 1;
            }
            tok!(start, i, 3u8);
            // (`3f32`: the suffix is a name the prelude selection looks for, as `f32` written as a type is)
            let hex = s[start] == b'0' && start + 1 < i && matches!(s[start + 1], b'x' | b'X' | b'b' | b'o');
            if i - start > 3 && !hex && matches!(&s[i - 3..i], b"f32" | b"f64") {
                if let Some(cur) = st.cur.as_mut() {
                    cur.2.push(((i - 3) as u32, i as u32));
                }
            }
            continue;
        }
        if c == b'\'' {
            // 文字リテラル or ライフタイム
            if i + 1 < n && s[i + 1] == b'\\' {
                i += 2;
                while i < n && s[i] != b'\'' {
                    i += 1;
                }
                i += 1;
                tok!(start, i.min(n), 1u8);
                continue;
            }
            let ch_len = utf8_len(s.get(i + 1).copied().unwrap_or(0));
            if i + 1 + ch_len < n && s[i + 1 + ch_len] == b'\'' {
                i += 2 + ch_len;
                tok!(start, i, 1u8);
                continue;
            }
            i += 1;
            while i < n && is_ident_cont(s[i]) {
                i += 1;
            }
            tok!(start, i, 4u8);
            continue;
        }
        // 記号 (1 byte ずつ。直前が空白かどうかもハッシュに入れて `&&` と `& &` を区別)
        i += 1;
        // impl ブロックの閉じ括弧はどのアイテムにも属さない
        if c == b'}' && impl_header.is_some() && depth == 1 {
            if let Some((sp, ..)) = st.cur {
                // (attributes with no item after them, `impl Foo { #[stable(..)] }`: rustc's error at the first)
                let sp = sp as usize;
                if attrs_only(std::str::from_utf8(&s[sp..start]).unwrap_or("")) {
                    let col = std::str::from_utf8(&s[s[..sp].iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1)..sp]).map_or(1, |l| l.chars().count() + 1);
                    return Err(format!("{}:{col}: expected item after attributes", st.line_at(sp)));
                }
                return Err(format!(
                    "{}:1: unterminated item in impl block",
                    st.line_at(start)
                ));
            }
            depth = 0;
            base = 0;
            impl_header = None;
            continue;
        }
        if c == b'#' && depth == base && st.cur.is_none() {
            let mut j = i;
            while j < n && s[j].is_ascii_whitespace() {
                j += 1;
            }
            inner_attr = j < n && s[j] == b'!';
        }
        tok!(start, i, 5u8);
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            // (generic arguments in an item's head may hold a block: `fn f() -> Foo<{ N }> {` /
            // `impl X for Foo<{ N }> {` do not end at the block's `}`)
            b'<' if depth == base && !semi_only => angle += 1,
            b'>' if depth == base && !semi_only && !(start > 0 && matches!(s[start - 1], b'-' | b'=')) => angle -= 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!(
                        "{}:1: unbalanced `{}`",
                        st.line_at(start),
                        c as char
                    ));
                }
            }
            _ => {}
        }
        let header_end = c == b'{' && depth == 1 && impl_pending && angle <= 0;
        let attr_end = inner_attr && c == b']' && depth == base;
        let item_end = depth == base && (c == b';' || (c == b'}' && !semi_only && angle <= 0));
        if header_end || attr_end || item_end {
            let (sp, ln, idents) = st.cur.take().unwrap();
            let mut h = KeyHasher::new();
            let kind = if attr_end {
                inner_attr = false;
                ItemKind::InnerAttr
            } else if header_end {
                if is_trait {
                    ItemKind::TraitHeader
                } else {
                    ItemKind::ImplHeader
                }
            } else if let Some(hi) = impl_header {
                // メンバのハッシュにはヘッダ (= self 型 / trait 名) を含める
                h.add(&items[hi as usize].hash);
                if is_trait {
                    ItemKind::TraitMember(hi)
                } else {
                    ItemKind::ImplMember(hi)
                }
            } else {
                ItemKind::Normal
            };
            h.add(&st.buf[..]);
            st.buf.clear();
            st.seen_ident = false;
            st.n_head = 0;
            semi_only = false;
            // (`impl X;` のような壊れたアイテムの後で、次の `{` をヘッダにしない: アイテムの境界では状態を戻す)
            impl_pending = false;
            angle = 0;
            if header_end {
                impl_pending = false;
                impl_header = Some(items.len() as u32);
                base = 1;
            }
            items.push(SrcItem {
                start: sp,
                end: i,
                line: ln,
                hash: h.finish(),
                idents,
                kind,
            });
        }
    }
    // (E0585: out of line, so the loop above compiles as it did)
    if !docs.is_empty() {
        dangling_docs(s, docs)?;
    }
    if impl_header.is_some() {
        return Err("unterminated impl block".into());
    }
    if let Some((sp, ln, ..)) = st.cur {
        // (attributes with no item after them at the end: those of a `macro_rules!` that expansion took out
        // (`#[macro_export] macro_rules! m { .. }` last in a file))
        let mut rest = std::str::from_utf8(&s[sp as usize..]).unwrap_or("").trim_start();
        while let Some(r) = rest.strip_prefix("#[") {
            let mut d = 1;
            let mut end = None;
            for (i, c) in r.char_indices() {
                match c {
                    '[' => d += 1,
                    ']' => {
                        d -= 1;
                        if d == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            match end {
                Some(e) => rest = r[e + 1..].trim_start(),
                None => break,
            }
        }
        if rest.is_empty() {
            return Ok(items);
        }
        // (a literal or a comment the file ends in: rustc's lexer reports it, at its start, before the delimiters)
        if let Some(e) = unterminated_literal(s) {
            return Err(e);
        }
        // (more delimiters opened than closed to the end: rustc's error at the end of the file, its last line)
        if unclosed_delims(rest.as_bytes()) {
            let src = std::str::from_utf8(s).unwrap_or("");
            let last = src.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()).map(|(i, _)| i + 1).last().unwrap_or(1);
            return Err(format!("{last}:1: this file contains an unclosed delimiter"));
        }
        return Err(format!(
            "{ln}:1: unterminated item starting here (byte {sp})"
        ));
    }
    Ok(items)
}

/// Whether b leaves `(` `[` `{` open at its end with every closer matching, outside strings, chars and comments (roughly: a lifetime's `'`
/// is told from a char's by the closing quote two or three bytes on)
fn unclosed_delims(b: &[u8]) -> bool {
    let mut stack: Vec<u8> = vec![];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            // (block comments nest: `/* a /* b */ } */`)
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                let mut depth = 1;
                while i + 1 < b.len() && depth > 0 {
                    if b[i] == b'/' && b[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                continue;
            }
            // (a raw string `r#".."#` (or `br"..")`: its end as the lexer reads it, quotes inside and all)
            b'r' | b'b' if (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) && matches!(b.get(i + 1), Some(b'"' | b'#' | b'r')) => {
                match string_lit(b, i) {
                    Some(e) if e > i => {
                        i = e;
                        continue;
                    }
                    _ => {}
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'\'' if b.get(i + 2) == Some(&b'\'') || (b.get(i + 1) == Some(&b'\\') && b.get(i + 3) == Some(&b'\'')) => {
                i += if b[i + 1] == b'\\' { 3 } else { 2 };
            }
            c @ (b'(' | b'[' | b'{') => stack.push(c),
            c @ (b')' | b']' | b'}') => {
                let open = match c {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                // (a closer that does not match: a mismatched delimiter, reported where the item starts)
                if stack.pop() != Some(open) {
                    return false;
                }
            }
            _ => {}
        }
        i += 1;
    }
    !stack.is_empty()
}

fn utf8_len(b: u8) -> usize {
    match b {
        0xf0.. => 4,
        0xe0.. => 3,
        0xc0.. => 2,
        _ => 1,
    }
}

/// Whether a text is outer attributes alone (`#[..]` groups, whitespace and comments between), at least one
#[cold]
fn attrs_only(text: &str) -> bool {
    let mut rest = text.trim_start();
    if !rest.starts_with("#[") {
        return false;
    }
    while let Some(r) = rest.strip_prefix("#[") {
        let mut d = 1;
        let Some(e) = r.char_indices().find_map(|(i, c)| {
            match c {
                '[' => d += 1,
                ']' => d -= 1,
                _ => {}
            }
            (d == 0).then_some(i)
        }) else {
            return false;
        };
        rest = r[e + 1..].trim_start();
        while rest.starts_with("//") {
            rest = rest[rest.find('\n').unwrap_or(rest.len())..].trim_start();
        }
    }
    rest.is_empty()
}

/// rustc's lexer error for a string, a raw string or a block comment that the file ends in, at its start: E0765
/// "unterminated double quote string" (E0766 for a byte string), E0748 "unterminated raw string", E0758
/// "unterminated block comment". Read only when the items do not split (an error is reported anyway)
#[cold]
pub fn unterminated_literal(s: &[u8]) -> Option<String> {
    let n = s.len();
    let at = |i: usize, m: &str| {
        let line = 1 + s[..i].iter().filter(|&&c| c == b'\n').count();
        let col = std::str::from_utf8(&s[s[..i].iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1)..i]).map_or(1, |l| l.chars().count() + 1);
        Some(format!("{line}:{col}: {m}"))
    };
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    let mut i = 0;
    while i < n {
        let c = s[i];
        if c == b'/' && s.get(i + 1) == Some(&b'/') {
            i += s[i..].iter().position(|&c| c == b'\n').unwrap_or(n - i);
        } else if c == b'/' && s.get(i + 1) == Some(&b'*') {
            let (st, mut d) = (i, 0u32);
            loop {
                if i + 1 >= n {
                    // (`/**` and `/*!` open a doc comment, which rustc names so; `/***` and `/**/` do not)
                    let t = &s[st..];
                    let doc = t.starts_with(b"/*!") || (t.starts_with(b"/**") && !t.starts_with(b"/***") && !t.starts_with(b"/**/"));
                    return at(st, if doc { "unterminated block doc-comment (E0758)" } else { "unterminated block comment (E0758)" });
                }
                if s[i] == b'/' && s[i + 1] == b'*' {
                    d += 1;
                    i += 2;
                } else if s[i] == b'*' && s[i + 1] == b'/' {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == b'\'' {
            // (a char literal, `'x'` / `'\''`, or a lifetime)
            let close = if s.get(i + 1) == Some(&b'\\') { s[i + 3.min(n - i)..].iter().position(|&c| c == b'\'').map(|p| i + 3 + p) } else { Some(i + 1 + s.get(i + 1).map_or(1, |&b| utf8_len(b))).filter(|&e| s.get(e) == Some(&b'\'')) };
            i = match close {
                Some(e) => e + 1,
                None => i + 1,
            };
        } else if ident(c) {
            let (st, raw) = (i, c == b'r' || ((c == b'b' || c == b'c') && s.get(i + 1) == Some(&b'r')));
            let mut j = if raw && c != b'r' { i + 2 } else { i + 1 };
            if raw && (s.get(j) == Some(&b'#') || s.get(j) == Some(&b'"')) {
                let h = s[j..].iter().take_while(|&&c| c == b'#').count();
                if s.get(j + h) == Some(&b'"') {
                    j += h + 1;
                    let end = (j..n).find(|&k| s[k] == b'"' && s.len() >= k + 1 + h && s[k + 1..k + 1 + h].iter().all(|&c| c == b'#'));
                    match end {
                        Some(k) => i = k + 1 + h,
                        None => return at(st, "unterminated raw string (E0748)"),
                    }
                    continue;
                }
            }
            if (c == b'b' || c == b'c') && s.get(i + 1) == Some(&b'"') {
                i += 1;
                continue;
            }
            if c == b'b' && s.get(i + 1) == Some(&b'\'') {
                i += 1;
                continue;
            }
            while j < n && ident(s[j]) {
                j += 1;
            }
            i = j;
        } else if c == b'"' {
            let byte = i > 0 && s[i - 1] == b'b';
            let st = if byte { i - 1 } else { i };
            let mut j = i + 1;
            while j < n && s[j] != b'"' {
                j += if s[j] == b'\\' { 2 } else { 1 };
            }
            if j >= n {
                return at(st, if byte { "unterminated double quote byte string (E0766)" } else { "unterminated double quote string (E0765)" });
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    None
}

/// string_lit, looked for only at the bytes a string, byte or C string literal starts with (`"`, `b`, `r`, `c`): the
/// others are most tokens, and string_lit is not inlined
#[inline(always)]
fn string_lit_at(s: &[u8], i: usize) -> Option<usize> {
    if matches!(s.get(i), Some(b'"' | b'b' | b'r' | b'c')) { string_lit(s, i) } else { None }
}

/// 位置 i から文字列リテラルが始まっていればその終端を返す。
fn string_lit(s: &[u8], mut i: usize) -> Option<usize> {
    let n = s.len();
    // 接頭辞
    if i < n && (s[i] == b'b' || s[i] == b'c') {
        if i + 1 < n && (s[i + 1] == b'"' || s[i + 1] == b'r') {
            i += 1;
        } else if s[i] == b'b' && i + 1 < n && s[i + 1] == b'\'' {
            // b'x'
            let mut j = i + 2;
            if j < n && s[j] == b'\\' {
                j += 1;
            }
            j += 1;
            while j < n && s[j] != b'\'' {
                j += 1;
            }
            return Some((j + 1).min(n));
        } else {
            return None;
        }
    }
    if i < n && s[i] == b'r' {
        let mut j = i + 1;
        let mut hashes = 0;
        while j < n && s[j] == b'#' {
            hashes += 1;
            j += 1;
        }
        if j >= n || s[j] != b'"' {
            return None;
        }
        j += 1;
        loop {
            if j >= n {
                return Some(n);
            }
            if s[j] == b'"'
                && s.len() >= j + 1 + hashes
                && s[j + 1..j + 1 + hashes].iter().all(|&c| c == b'#')
            {
                return Some(j + 1 + hashes);
            }
            j += 1;
        }
    }
    if i < n && s[i] == b'"' {
        let mut j = i + 1;
        while j < n && s[j] != b'"' {
            if s[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        return Some((j + 1).min(n));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// has_inline_item: an item on the same line after `{` / `;` / `}`, also after attributes; an attribute with a
    /// string or brackets in it (`#[doc = "]"]`) is taken as followed by an item (the full preparation decides)
    #[test]
    fn inline_item_after_attributes() {
        assert!(has_inline_item("fn outer() { fn inner() {} }"));
        assert!(has_inline_item("fn outer() { #[inline] fn inner() {} }"));
        assert!(has_inline_item("fn outer() { #[doc = \"]\"] fn inner() {} inner() }"));
        assert!(!has_inline_item("fn outer() { #[allow(unused)] let x = 1; }"));
        assert!(!has_inline_item("fn outer() {\n    let x = 1;\n}"));
    }

    /// Edition 2021's reserved prefixes: an identifier right before `#` / `"` / `'`, and what only looks like one
    #[test]
    fn reserved_prefixes() {
        let bad = |s: &str| unknown_prefix(s).unwrap_or_default();
        assert_eq!(bad("m!(foo#bar);"), "1:4: prefix `foo` is unknown");
        assert_eq!(bad("x\nm!(foo\"s\");"), "2:4: prefix `foo` is unknown");
        assert_eq!(bad("m!(foo'b');"), "1:4: prefix `foo` is unknown");
        assert_eq!(bad("m!('prefix#lt);"), "1:5: prefix `prefix` is unknown");
        assert_eq!(bad("m!($x#y);"), "1:5: prefix `x` is unknown");
        for ok in [
            "let s = \"abc\"; let c = 'x'; let t = b\"by\"; let u = b'y';",
            "let r = r#\"a \"q\" foo#bar\"#; let rr = r##\"x\"#y\"##; let raw = br\"z\"; let cs = c\"c\";",
            "fn f<'a>(x: &'a str) -> &'a str { x } let e = '\\n'; let q = '\\'';",
            "// foo#bar \"x\"y\n/* a\"b /* foo#bar */ */ let n = 1u8;",
            "m!(r#let#foo); let r#match = 1; m!($x # y); let s = \"foo#bar\";",
            "m!(\"x\"tag#tail); m!('c'tag#t); m!('r#a); fn g<'a>(_: &'a u8) {} let n = 'a';",
        ] {
            assert_eq!(unknown_prefix(ok), None, "{ok}");
        }
    }

    fn hashes(src: &str) -> Vec<Key> {
        split(src).unwrap().iter().map(|i| i.hash).collect()
    }

    /// A path through glob re-exports names the item they reach: in a dependency crate, `crate::uid_t` with the
    /// root's `pub use unix::*;`, and `mylibc::passwd` from outside through two globs (`pub use self::linux::*;` in
    /// unix). libc 0.2: `crate::gid_t` in `unix/linux_like` was "cannot find type"
    #[test]
    fn paths_through_glob_reexports() {
        let src = "mod mylibc {\n    pub mod unix {\n        pub type uid_t = u32;\n        pub mod linux {\n            pub struct passwd { pub pw_uid: crate::uid_t }\n            pub fn uid(p: &passwd) -> crate::uid_t { p.pw_uid }\n        }\n        pub use self::linux::*;\n    }\n    pub use unix::*;\n}\nfn main() {\n    let p = mylibc::passwd { pw_uid: 1 };\n    let _u: mylibc::uid_t = mylibc::uid(&p);\n}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(!out.contains("crate::uid_t") && !out.contains("crate :: uid_t"), "{out}");
        assert!(out.contains("pw_uid: mylibc__unix__uid_t"), "{out}");
        assert!(out.contains("let p = mylibc__unix__linux__passwd {"), "{out}");
        assert!(out.contains("let _u: mylibc__unix__uid_t = mylibc__unix__linux__uid(&p);"), "{out}");
    }

    /// A bare name a module's globs pass on from several levels down: libc's unix uses `time_t`, which
    /// `linux_like/linux/gnu/b64` defines, through `pub use self::linux_like::*;` and the globs below it
    #[test]
    fn bare_name_through_nested_globs() {
        let src = "mod mylibc {\n    pub mod unix {\n        pub mod linux {\n            pub mod b64 {\n                pub type time_t = i64;\n            }\n            pub use self::b64::*;\n        }\n        pub use self::linux::*;\n        pub struct timeval { pub tv_sec: time_t }\n    }\n    pub use unix::*;\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(out.contains("tv_sec: mylibc__unix__linux__b64__time_t"), "{out}");
    }

    /// A named import of what a glob re-exports: libc's prelude has `pub(crate) use crate::{size_t, ..};` with
    /// size_t from the root's `pub use unix::*;`, and `use crate::prelude::*;` in unix/linux_like
    #[test]
    fn named_import_of_a_glob_reexport() {
        let src = "mod mylibc {\n    mod prelude {\n        pub(crate) use crate::{size_t};\n    }\n    pub mod unix {\n        pub type size_t = usize;\n        pub mod linux_like {\n            use crate::prelude::*;\n            pub struct iovec { pub iov_len: size_t }\n        }\n        pub use self::linux_like::*;\n    }\n    pub use unix::*;\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(out.contains("iov_len: mylibc__unix__size_t"), "{out}");
    }

    /// A `use` path whose first module came by a glob: libc's new/mod.rs has `pub(crate) use linux_uapi::*;` and then
    /// `pub use linux::types::*;`, linux being linux_uapi's (`crate::__s64` in unix/linux_like)
    #[test]
    fn use_path_through_a_globbed_module() {
        let src = "mod mylibc {\n    mod new {\n        mod linux_uapi {\n            pub(crate) mod linux {\n                pub(crate) mod types {\n                    pub type __s64 = i64;\n                }\n            }\n        }\n        pub(crate) use linux_uapi::*;\n        pub use linux::types::*;\n    }\n    pub use new::*;\n    pub struct statx_timestamp { pub tv_sec: crate::__s64 }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(out.contains("tv_sec: mylibc__new__linux_uapi__linux__types____s64"), "{out}");
    }

    /// A glob of a path through modules that globs bring in, written before those globs: libc's can/bcm.rs has
    /// `pub use crate::linux::can::*;`, linux coming by the root's `pub use new::*;` and new's
    /// `pub(crate) use linux_uapi::*;`
    #[test]
    fn glob_path_through_later_globs() {
        let src = "mod mylibc {\n    pub mod bcm {\n        pub use crate::linux::can::*;\n        pub struct frame { pub id: canid_t }\n    }\n    mod new {\n        mod linux_uapi {\n            pub(crate) mod linux {\n                pub(crate) mod can {\n                    pub type canid_t = u32;\n                }\n            }\n        }\n        pub(crate) use linux_uapi::*;\n    }\n    pub use new::*;\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(out.contains("id: mylibc__new__linux_uapi__linux__can__canid_t"), "{out}");
    }

    /// Types and fns are apart: in libc's unix, `fn stat(path: *const c_char, buf: *mut stat)` beside the `struct stat`
    /// a glob brings in from linux_like. The `stat` of `*mut stat` is the struct, not the fn's renamed name
    #[test]
    fn a_type_position_does_not_take_a_fn_of_the_name() {
        let src = "mod mylibc {\n    pub mod unix {\n        pub mod linux {\n            pub struct stat { pub st_size: i64 }\n        }\n        pub use self::linux::*;\n        extern \"C\" {\n            pub fn stat(path: *const u8, buf: *mut stat) -> i32;\n        }\n        pub fn size(s: &stat) -> i64 { s.st_size }\n    }\n    pub use unix::*;\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(out.contains("buf: *mut mylibc__unix__linux__stat"), "{out}");
        assert!(out.contains("s: &mylibc__unix__linux__stat"), "{out}");
        assert!(out.contains("pub fn mylibc__unix__stat("), "{out}");
    }

    /// A parameter named as its type, `fn mktime(tm: *mut tm)` (libc's unix, the struct tm from linux_like by a glob):
    /// the type is renamed, the parameter's name is not
    #[test]
    fn parameter_named_as_its_type() {
        let src = "mod mylibc {\n    pub mod unix {\n        pub mod linux_like {\n            pub struct tm { pub tm_sec: i32 }\n        }\n        pub use self::linux_like::*;\n        extern \"C\" {\n            pub fn mktime(tm: *mut tm) -> i64;\n            pub fn gmtime(t: *const i64) -> *mut tm;\n        }\n    }\n    pub use unix::*;\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(out.contains("mktime(tm: *mut mylibc__unix__linux_like__tm)"), "{out}");
        assert!(out.contains("-> *mut mylibc__unix__linux_like__tm"), "{out}");
    }

    /// A path through a module imported by name: glibc's `bits` has `pub(crate) use super::sysdeps::bits::{types};`
    /// and signal.rs `pub use crate::new::glibc::bits::types::siginfo_t::*;` (libc's `crate::siginfo_t`)
    #[test]
    fn glob_path_through_a_module_imported_by_name() {
        let src = "mod mylibc {\n    pub mod glibc {\n        mod sysdeps {\n            pub(crate) mod bits {\n                pub(crate) mod types {\n                    pub(crate) mod siginfo_t {\n                        pub struct siginfo_t { pub si_signo: i32 }\n                    }\n                }\n            }\n        }\n        mod bits {\n            pub(crate) use super::sysdeps::bits::{types};\n        }\n        pub(crate) mod signal {\n            pub use crate::glibc::bits::types::siginfo_t::*;\n        }\n        pub use signal::*;\n    }\n    pub use glibc::*;\n    pub fn signo(i: *const crate::siginfo_t) -> i32 { 0 }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(out.contains("i: *const mylibc__glibc__sysdeps__bits__types__siginfo_t__siginfo_t"), "{out}");
    }

    /// A path in a type's place does not take a fn of the name either: `fn sysinfo(info: *mut crate::sysinfo)` in
    /// libc's linux_l4re_shared, the struct sysinfo coming by another glob
    #[test]
    fn a_type_path_does_not_take_a_fn_of_the_name() {
        let src = "mod mylibc {\n    pub mod unix {\n        pub mod shared {\n            extern \"C\" {\n                pub fn sysinfo(info: *mut crate::sysinfo) -> i32;\n            }\n        }\n        pub mod gnu {\n            pub struct sysinfo { pub uptime: i64 }\n        }\n        pub use self::shared::*;\n        pub use self::gnu::*;\n    }\n    pub use unix::*;\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylibc".to_string()]).unwrap();
        assert!(out.contains("info: *mut mylibc__unix__gnu__sysinfo"), "{out}");
    }

    /// A fn's generic parameter after `->` stays the parameter beside an import of its name (`use std::cell::Cell as C;`
    /// and `fn id<C>(x: C) -> C`), while a value parameter there does not hide a type (`fn mktime(tm: *mut tm)`)
    #[test]
    fn generic_parameter_after_arrow_stays_the_parameter() {
        let src = "use std::cell::Cell as C;\nfn id<C>(x: C) -> C {\n    x\n}\nfn main() {\n    let c = C::new(1);\n    let _ = id(c);\n}\n";
        let out = rename_module_collisions_in(src, &[]).unwrap();
        assert!(out.contains("fn id<C>(x: C) -> C {"), "{out}");
    }

    /// A fn called after `&` is a value, not a type's place: `&pcg32(&mut state)` (rand_core's `seed_from_u64`, the
    /// private fn renamed with its module) calls the fn
    #[test]
    fn a_call_after_a_borrow_is_the_fn() {
        let src = "mod mylib {\n    pub mod seedable {\n        pub fn seed(state: &mut u64) -> u8 {\n            let b = &pcg32(state);\n            b[0]\n        }\n        fn pcg32(state: &mut u64) -> [u8; 4] {\n            *state += 1;\n            [*state as u8; 4]\n        }\n    }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylib".to_string()]).unwrap();
        assert!(out.contains("let b = &mylib__seedable__pcg32(state);"), "{out}");
    }

    /// A fn named like a primitive type is a value (#254): getrandom's `pub fn u32() -> Result<u32, Error>` and
    /// fastrand's `u8` (by the root's glob) leave the type, a number's suffix and a path through the type as they are;
    /// a call and a call's argument by itself are the fn
    #[test]
    fn a_fn_named_like_a_primitive_leaves_the_type() {
        let src = "mod mylib {\n    pub mod global_rng {\n        pub fn u8(n: u8) -> u8 { n }\n    }\n    pub use global_rng::*;\n    pub struct Error;\n    pub fn u32() -> Result<u32, Error> {\n        let b: [u8; 2] = [0u8, 0x1fu8];\n        Ok(u32::MAX - u8(b[1]) as u32)\n    }\n    pub fn bytes(v: &[u8]) -> Vec<u8> { v.iter().copied().map(u8).collect() }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["mylib".to_string()]).unwrap();
        assert!(out.contains("pub fn mylib__global_rng__u8(n: u8) -> u8 { n }"), "{out}");
        assert!(out.contains("pub fn mylib__u32() -> Result<u32, mylib__Error> {"), "{out}");
        assert!(out.contains("let b: [u8; 2] = [0u8, 0x1fu8];"), "{out}");
        assert!(out.contains("Ok(u32::MAX - mylib__global_rng__u8(b[1]) as u32)"), "{out}");
        assert!(out.contains("pub fn mylib__bytes(v: &[u8]) -> Vec<u8> { v.iter().copied().map(mylib__global_rng__u8).collect() }"), "{out}");
    }

    /// `self` first in a `use` tree names the tree's prefix (#254): `use super::Sign::{self, Plus};` brings the enum
    /// (num-bigint), `use crate::util::{alphabet::{self}};` the module
    #[test]
    fn self_in_a_use_tree_is_its_prefix() {
        let mut items = vec![];
        let src = b"use super::Sign::{self, Minus as M, Plus}; use crate::util::{alphabet::{self as a, Unit}};";
        let toks = tokenize(src);
        let end = toks.len();
        use_walk(&toks, src, 1, end, vec![], &mut items);
        let names: Vec<(String, String)> = items
            .iter()
            .map(|(p, l)| (p.iter().map(|x| String::from_utf8_lossy(x).into_owned()).collect::<Vec<_>>().join("::"), String::from_utf8_lossy(l).into_owned()))
            .collect();
        let has = |p: &str, l: &str| names.iter().any(|n| n.0 == p && n.1 == l);
        assert!(has("super::Sign", "Sign") && has("super::Sign::Minus", "M") && has("super::Sign::Plus", "Plus"), "{names:?}");
        let src = "mod mylib {\n    pub mod util {\n        pub mod alphabet {\n            pub struct Unit(pub u8);\n        }\n    }\n    pub mod dense {\n        use crate::util::{alphabet::{self, Unit}};\n        pub fn f(u: Unit) -> alphabet::Unit { u }\n    }\n}\nfn main() {}\n";
        let out = flatten_modules_in(src, &["mylib".to_string()]).unwrap();
        assert!(out.contains("fn mylib__dense__f(u: mylib__util__alphabet__Unit) -> mylib__util__alphabet__Unit"), "{out}");
    }

    /// A path in a `use` tree's braces goes on from the tree's prefix, though a `use` of the module imports its last
    /// name (#254, regex-syntax's hir: `use crate::unicode;` and `pub use crate::{hir::x, unicode::CaseFoldError};`)
    #[test]
    fn a_module_alias_is_not_expanded_in_a_use_tree() {
        let src = "mod unicode {\n    pub struct CaseFoldError;\n}\nmod hir {\n    use crate::unicode;\n    pub use crate::{hir::v::f, unicode::CaseFoldError};\n    mod v { pub fn f() {} }\n    fn g() -> unicode::CaseFoldError { unicode::CaseFoldError }\n}\nfn main() {}\n";
        let out = expand_module_aliases(src).unwrap();
        assert!(out.contains("pub use crate::{hir::v::f, unicode::CaseFoldError};"), "{out}");
        assert!(out.contains("fn g() -> crate::unicode::CaseFoldError { crate::unicode::CaseFoldError }"), "{out}");
        // (a group right after `use` names from the scope, as the `use` does: the alias is followed in it)
        let src = "mod p {\n    pub mod q { pub fn f() {} }\n}\nmod m {\n    use crate::p as a;\n    use {a::q as b, a::{q as c}};\n    fn g() { b::f() }\n}\nfn main() {}\n";
        let out = expand_module_aliases(src).unwrap();
        assert!(out.contains("use {crate::p::q as b, crate::p::{q as c}};"), "{out}");
    }

    /// A `self` entry brings the type namespace only (#254 review): `use crate::a::m::{self};` is the module m, not a's
    /// fn m beside it, so `m()` is the one a glob brings
    #[test]
    fn a_self_import_brings_no_value() {
        let src = "mod d {\n    pub mod a {\n        pub mod m { pub fn inner() -> u8 { 3 } }\n        pub fn m() -> u8 { 1 }\n    }\n    pub mod b {\n        pub fn m() -> u8 { 2 }\n    }\n    pub mod user {\n        use crate::a::m::{self};\n        use crate::b::*;\n        pub fn run() -> u8 { m() * 10 + m::inner() }\n    }\n}\nfn main() {}\n";
        let out = flatten_modules_in(src, &["d".to_string()]).unwrap();
        assert!(out.contains("{ d__b__m() * 10 + d__a__m__inner() }"), "{out}");
    }

    /// A fn named like a primitive is a value in an expression's place (#254 review): `let f = u32;` takes the fn, the
    /// annotation and a binding among generics the type
    #[test]
    fn a_fn_named_like_a_primitive_as_a_value() {
        let src = "mod d {\n    pub fn u32() -> usize { 1 }\n    pub fn run() -> usize {\n        let f = u32;\n        let g: fn() -> usize = u32;\n        let x: u32 = 4;\n        let i: Option<Box<dyn Iterator<Item = u32>>> = None;\n        f() + g() + x as usize + i.map_or(0, |_| 1)\n    }\n    pub fn cmp(f: fn() -> usize, n: u32) -> bool {\n        f == u32 as fn() -> usize && (u32 != f) && n == u32::MAX && n as u32 == 4\n    }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["d".to_string()]).unwrap();
        assert!(out.contains("let f = d__u32;"), "{out}");
        assert!(out.contains("let g: fn() -> usize = d__u32;"), "{out}");
        assert!(out.contains("let x: u32 = 4;"), "{out}");
        assert!(out.contains("Iterator<Item = u32>"), "{out}");
        // (a comparison's operands, `f == u32` / `u32 != f`; a path's head and a cast's type stay the type's)
        assert!(out.contains("f == d__u32 as fn() -> usize && (d__u32 != f) && n == u32::MAX && n as u32 == 4"), "{out}");
    }

    /// A block's import named like a primitive shadows it in a type's place too (#254 review):
    /// `use crate::Byte as u8;` makes the annotation `u8` the struct, as the call
    #[test]
    fn a_block_import_named_like_a_primitive_shadows_it() {
        let src = "mod d {\n    pub struct Byte(pub u8);\n    pub fn run() {\n        use crate::Byte as u8;\n        let _: u8 = u8(1);\n    }\n    pub fn other() -> u8 { 2u8 }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["d".to_string()]).unwrap();
        assert!(out.contains("let _: d__Byte = d__Byte(1);"), "{out}");
        assert!(out.contains("pub fn d__other() -> u8 { 2u8 }"), "{out}");
    }

    /// A tuple variant's fields are types, not a call's arguments, though its name is lowercase (#254 review):
    /// `enum E { lower(u8) }` beside `fn u8`, while `.map(u8)` passes the fn
    #[test]
    fn a_tuple_variant_of_a_primitive_is_no_call() {
        let src = "mod d {\n    pub fn u8(x: u8) -> u8 { x }\n    pub enum E { lower(u8), Upper(u8, char) }\n    pub fn f(v: Vec<u8>) -> Vec<u8> { v.into_iter().map(u8).collect() }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["d".to_string()]).unwrap();
        assert!(out.contains("pub enum d__E { lower(u8), Upper(u8, char) }"), "{out}");
        assert!(out.contains(".map(d__u8)"), "{out}");
    }

    /// A block's import is looked up in the place's namespaces (#254 review): `use crate::a::E::{self};` in a body
    /// brings the type E, so `E::V` is a's enum and the call `E()` the fn a glob brings
    #[test]
    fn a_block_self_import_is_no_value() {
        let src = "mod d {\n    pub mod a {\n        pub enum E { V }\n        pub fn E() -> u8 { 1 }\n    }\n    pub mod b {\n        pub fn E() -> u8 { 2 }\n    }\n    pub mod user {\n        use crate::b::*;\n        pub fn run() -> u8 {\n            use crate::a::E::{self};\n            let _v = E::V;\n            let f = E;\n            E() + f()\n        }\n    }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["d".to_string()]).unwrap();
        assert!(out.contains("let _v = d__a__E::V;\n            let f = d__b__E;\n            d__b__E() + f()\n"), "{out}");
    }

    /// A struct literal in a value's place names the type (rustc's tests/ui lint/use-redundant): a block's
    /// `use m1::S;` beside the globs `use m1::*; use m2::*;` makes `let s = S {};` m1's S, as `=> S {}` and
    /// `return S {}`; a comparison's operand before an `if`'s block is still a value
    #[test]
    fn a_struct_literal_takes_a_block_import_of_the_type() {
        let src = "mod d {\n    mod m1 { pub struct S {} pub const B: u8 = 1; }\n    mod m2 { pub struct S {} pub const B: u8 = 2; }\n    use m1::*;\n    use m2::*;\n    pub fn run(x: u8) -> u8 {\n        use m1::S;\n        use m1::B;\n        let _s = S {};\n        let _t = match x { 0 => S {}, _ => return 0 };\n        if x == B { 1 } else { 2 }\n    }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["d".to_string()]).unwrap();
        assert!(out.contains("let _s = d__m1__S {};"), "{out}");
        assert!(out.contains("0 => d__m1__S {}"), "{out}");
        assert!(out.contains("if x == d__m1__B { 1 }"), "{out}");
        // (a struct with fields, which only the type namespace has: CodeRabbit's case on #277)
        let src = "mod d {\n    pub mod a { pub struct E { pub x: u8 } }\n    pub fn run() { use crate::a::E; let e = E { x: 1 }; let _ = e.x; }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["d".to_string()]).unwrap();
        assert!(out.contains("let e = d__a__E { x: 1 };"), "{out}");
    }

    /// A source that starts with `(` (`(u8)` beside a renamed `fn u8`): the walk back of a call's argument stops at the
    /// first token (#254 review)
    #[test]
    fn a_paren_at_the_start_is_no_call_of_a_variant() {
        let src = "(u8);\nmod d {\n    pub fn u8() -> u8 { 1 }\n}\nfn main() {}\n";
        let _ = rename_module_collisions_in(src, &["d".to_string()]);
    }

    /// A dependency under another name (`dig:common=cc`, a renamed one, mizchi/zust#249): every module of dig sees
    /// `common` as cc (a path, `::common::..`, a `use` of an item and of a module's item, a re-export a dependent's path
    /// goes through), after its own names (a module of the library's name); the root of the source too (`:c=cc`)
    #[test]
    fn renamed_dependency_is_the_crate_in_each_module() {
        let src = "pub mod cc {\n    pub fn make() -> u32 { 1 }\n    pub mod inner {\n        pub fn hi() {}\n    }\n}\npub mod dig {\n    pub use common;\n    pub mod cc {\n        pub fn make() -> u32 { 2 }\n    }\n    mod sub {\n        use common::inner::hi;\n        pub fn f() -> u32 {\n            hi();\n            common::make() + ::common::make() + crate::cc::make()\n        }\n    }\n}\nmod m {\n    pub fn g() -> u32 {\n        c::make() + dig::common::make()\n    }\n}\nfn main() {}\n";
        let crates: Vec<String> = ["cc", "dig", "dig:common=cc", ":c=cc"].iter().map(|s| s.to_string()).collect();
        let out = rename_module_collisions_in(src, &crates).unwrap();
        assert!(out.contains("            cc__inner__hi();\n            cc__make() + cc__make() + dig__cc__make()"), "{out}");
        assert!(out.contains("        cc__make() + cc__make()\n"), "{out}");
        // (without the entry, `common` is no module)
        let out = rename_module_collisions_in(src, &crates[..2]).unwrap();
        assert!(out.contains("common::make()"), "{out}");
        // (a bin's name for a crate over another crate of that name, which the bin does not depend on itself)
        let src = "pub mod cc {\n    pub fn make() -> u32 { 1 }\n}\npub mod other {\n    pub fn make() -> u32 { 2 }\n}\nfn main() {\n    let _ = other::make();\n}\n";
        let crates: Vec<String> = ["cc", "other", ":other=cc"].iter().map(|s| s.to_string()).collect();
        let out = rename_module_collisions_in(src, &crates).unwrap();
        assert!(out.contains("let _ = cc__make();"), "{out}");
    }

    fn ren(src: &str) -> String {
        rename_module_collisions_in(src, &["d".to_string()]).unwrap_or_else(|| src.to_string())
    }

    /// Review of #147: many glob providers before the one with the name (no cut-off at 64 modules)
    #[test]
    fn glob_search_has_no_size_cutoff() {
        let mut src = String::from("mod d {\n");
        for i in 0..70 {
            src.push_str(&format!("    pub mod e{i} {{}}\n"));
        }
        src.push_str("    pub mod deep { pub mod leaf { pub struct T; } pub use self::leaf::*; }\n    pub mod facade {\n");
        for i in 0..70 {
            src.push_str(&format!("        pub use crate::e{i}::*;\n"));
        }
        src.push_str("        pub use crate::deep::*;\n    }\n    pub fn f(_t: crate::facade::T) {}\n}\nfn main() {}\n");
        assert!(ren(&src).contains("_t: d__deep__leaf__T"));
    }

    /// Review of #147: a glob chain longer than any fixed depth, and one written in reverse order (each glob through
    /// a module the next one's glob brings in)
    #[test]
    fn long_and_reverse_glob_chains() {
        let mut src = String::from("mod d {\n    pub mod m40 { pub struct T; }\n");
        for i in (0..40).rev() {
            src.push_str(&format!("    pub mod m{i} {{ pub use crate::m{}::*; }}\n", i + 1));
        }
        src.push_str("    pub fn f(_t: crate::m0::T) {}\n}\nfn main() {}\n");
        assert!(ren(&src).contains("_t: d__m40__T"), "long chain");
        // (s9 globs s8::api::*, where s8's api comes by s8's glob of s7, .. down to s0)
        let mut src = String::from("mod d {\n    pub mod seed { pub mod api { pub struct T; } }\n");
        for i in (1..10).rev() {
            src.push_str(&format!("    pub mod s{i} {{ pub use crate::s{}::*; }}\n", i - 1));
        }
        src.push_str("    pub mod s0 { pub use crate::seed::*; }\n    pub mod top { pub use crate::s9::api::*; }\n    pub fn f(_t: crate::top::T) {}\n}\nfn main() {}\n");
        assert!(ren(&src).contains("_t: d__seed__api__T"), "reverse chain");
    }

    /// Review of #147: a lattice of globs (two modules a level, each globbing both of the next) is resolved once per
    /// module, not once per route
    #[test]
    fn glob_lattice_is_not_walked_per_route() {
        let mut src = String::from("mod d {\n    pub mod a30 { pub struct T; }\n    pub mod b30 { pub use crate::a30::T as U; }\n");
        for i in (0..30).rev() {
            src.push_str(&format!("    pub mod a{i} {{ pub use crate::a{n}::*; pub use crate::b{n}::*; }}\n    pub mod b{i} {{ pub use crate::a{n}::*; pub use crate::b{n}::*; }}\n", n = i + 1));
        }
        src.push_str("    pub fn f(_t: crate::a0::T) {}\n}\nfn main() {}\n");
        let t = std::time::Instant::now();
        let out = ren(&src);
        assert!(out.contains("_t: d__a30__T"));
        assert!(t.elapsed().as_secs() < 5);
    }

    /// Review of #147: two globs that give different items are ambiguous (E0659): a module path's segment, a named
    /// import's item and a bare name stay as written
    #[test]
    fn ambiguous_globs_are_left_as_written() {
        let src = "mod d {\n    pub mod p { pub mod api { pub struct T; } pub fn f() {} }\n    pub mod q { pub mod api { pub struct T; } pub fn f() {} }\n    pub mod facade { pub use crate::p::*; pub use crate::q::*; }\n    pub mod user {\n        use crate::facade::api::T as Alias;\n        use crate::facade::*;\n        pub fn g(_a: Alias) { f() }\n    }\n}\nfn main() {}\n";
        let out = ren(src);
        assert!(out.contains("_a: Alias"), "{out}");
        assert!(out.contains("{ f() }"), "{out}");
        // (the same item by two routes is one)
        let src = "mod d {\n    pub mod leaf { pub struct T; }\n    pub mod a { pub use crate::leaf::*; }\n    pub mod b { pub use crate::leaf::*; }\n    pub mod facade { pub use crate::a::*; pub use crate::b::*; }\n    pub fn g(_t: crate::facade::T) {}\n}\nfn main() {}\n";
        assert!(ren(src).contains("_t: d__leaf__T"));
    }

    /// Review of #147: `crate::api::T` walks from the crate root through its glob; `facade::api::T` through facade's
    #[test]
    fn module_paths_through_globs_from_the_root_and_a_facade() {
        let src = "pub mod pkg { pub mod api { pub struct T; } }\npub use pkg::*;\nmod other { pub struct T; }\nuse crate::api::T as Alias;\nfn g(_a: Alias, _o: other::T) {}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &[]).unwrap();
        assert!(out.contains("_a: pkg__api__T"), "{out}");
        let src = "mod d {\n    pub mod pkg { pub mod api { pub struct T; } }\n    pub mod facade { pub use crate::pkg::*; }\n    pub fn g(_t: crate::facade::api::T) {}\n}\nfn main() {}\n";
        assert!(ren(src).contains("_t: d__pkg__api__T"));
    }

    /// Review of #147: namespaces. A value import or a constant of the name is not the type in a type's place; a
    /// call is the value
    #[test]
    fn namespaces_of_imports_and_constants() {
        let src = "mod d {\n    pub mod funcs { pub fn make() -> u8 { 1 } }\n    pub mod types { pub struct T { pub x: u8 } }\n    pub mod user {\n        use crate::funcs::make as T;\n        use crate::types::*;\n        pub fn g(p: *const T) -> u8 { T() }\n    }\n    pub mod consts {\n        const T: u32 = 0;\n        use crate::types::*;\n        pub fn h(p: *const T) -> u32 { T }\n    }\n}\nfn main() {}\n";
        let out = ren(src);
        assert!(out.contains("g(p: *const d__types__T) -> u8 { d__funcs__make() }"), "{out}");
        assert!(out.contains("h(p: *const d__types__T) -> u32 { d__consts__T }"), "{out}");
    }

    /// Review of #147: the type's places `&'a T` and a parameter's `t: &t`; a call after a qualified path, `0 < dep::f()`
    #[test]
    fn reference_types_and_calls_after_paths() {
        let src = "mod d {\n    pub mod linux { pub struct stat { pub n: i64 } pub struct t; }\n    pub mod unix {\n        pub use crate::linux::*;\n        pub fn stat() -> i32 { 0 }\n        pub fn size<'a>(s: &'a stat, m: &'a mut stat) -> i64 { s.n + m.n }\n        pub fn f(t: &t) {}\n        pub fn less() -> bool { 0 < crate::unix::stat() }\n    }\n}\nfn main() {}\n";
        let out = ren(src);
        assert!(out.contains("s: &'a d__linux__stat, m: &'a mut d__linux__stat"), "{out}");
        assert!(out.contains("f(t: &d__linux__t)"), "{out}");
        assert!(out.contains("0 < d__unix__stat()"), "{out}");
    }

    /// A glob brings what is visible from where it is written: libc's unix/linux_like declares a private `mod linux`
    /// and new a `pub(crate) mod linux`; with both globbed at the root, `crate::linux` is new's, not ambiguous
    #[test]
    fn globs_bring_only_visible_items() {
        let src = "mod d {\n    pub mod unix { pub mod linux_like { mod linux { pub type canid_t = u8; } pub fn f() {} } pub use self::linux_like::*; }\n    pub mod new { mod linux_uapi { pub(crate) mod linux { pub(crate) mod can { pub type canid_t = u32; } } } pub(crate) use linux_uapi::*; }\n    pub use unix::*;\n    pub use new::*;\n    pub mod bcm { pub use crate::linux::can::*; pub struct frame { pub id: canid_t } }\n    pub mod child { use super::*; fn g() { helper() } }\n    fn helper() {}\n}\nfn main() {}\n";
        let out = ren(src);
        assert!(out.contains("id: d__new__linux_uapi__linux__can__canid_t"), "{out}");
        // (a child's `use super::*;` sees the parent's private items)
        assert!(out.contains("{ d__helper() }"), "{out}");
    }

    /// Review of #147 (2): a braced struct is a type only, so `T()` calls a globbed fn T; a call is in the value
    /// namespace (`facade::T()` with a type alias T and a fn T); a type alias's right-hand side is a type's place
    #[test]
    fn value_and_type_places() {
        let src = "mod d {\n    pub mod funcs { pub fn T() -> u8 { 1 } }\n    pub mod types { pub struct T { pub x: u8 } pub type U = u8; }\n    pub mod one {\n        pub struct T { pub x: u8 }\n        use crate::funcs::*;\n        pub fn g() -> u8 { T() }\n    }\n    pub mod facade { pub use crate::funcs::*; pub mod inner { pub type T = u8; } pub use self::inner::*; }\n    pub fn h() -> u8 { crate::facade::T() }\n    pub mod alias {\n        fn T() {}\n        use crate::types::*;\n        pub type A = T;\n    }\n}\nfn main() {}\n";
        let out = ren(src);
        assert!(out.contains("pub fn d__one__g() -> u8 { d__funcs__T() }"), "{out}");
        assert!(out.contains("-> u8 { d__funcs__T() }\n    pub mod alias"), "{out}");
        assert!(out.contains("pub type d__alias__A = d__types__T;"), "{out}");
    }

    /// Review of #147 (2): a lookup that meets one in progress (a cycle) is not kept as a miss: `a` globs `b` then
    /// `leaf`, `b` globs `a`; both `a::T` and `b::T` are leaf's
    #[test]
    fn a_cycle_does_not_cache_a_miss() {
        let src = "mod d {\n    pub mod leaf { pub struct T; }\n    pub mod a { pub use crate::b::*; pub use crate::leaf::*; }\n    pub mod b { pub use crate::a::*; }\n    pub fn f() { let _x = crate::a::T; let _y = crate::b::T; }\n}\nfn main() {}\n";
        let out = ren(src);
        assert!(out.contains("let _x = d__leaf__T; let _y = d__leaf__T;"), "{out}");
    }

    /// A path to a module's private item still names it (the test harness calls `b::foo()` for a private
    /// `#[test] fn foo`; whether it may is E0603's check); a glob still brings only what is visible
    /// (tests/ui test-attrs/issue-52557.rs: `use a::*; use b::*;` with b's `foo` private is a's `foo`)
    #[test]
    fn a_path_names_a_private_item() {
        let src = "mod a {\n    pub fn foo() -> bool { true }\n}\nmod b {\n    fn foo() {}\n}\nuse a::*;\nuse b::*;\npub fn conflict() { let _: bool = foo(); }\nfn run() { b::foo(); }\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &[]).unwrap();
        assert!(out.contains("let _: bool = a__foo();"), "{out}");
        assert!(out.contains("fn run() { b__foo(); }"), "{out}");
    }

    /// A const generic parameter (`fn read_words<W, const N: usize>`, `impl<const N: usize>`) is no item of the
    /// module: a module's `const N` elsewhere does not make it collide, and it keeps its name (rand_core 0.10's
    /// utils.rs became `rand_core__utils__N`, "cannot find constant")
    #[test]
    fn const_generic_parameter_is_not_a_module_constant() {
        let src = "mod utils {\n    pub fn read_words<W: Copy, const N: usize>(_s: &[u8]) -> [W; N] { loop {} }\n    pub struct A<const N: usize>;\n    impl<const N: usize> A<N> { pub fn n(&self) -> usize { N } }\n}\nmod block {\n    pub const N: usize = 2;\n    pub fn first<const N: usize>(a: [u8; N]) -> u8 { a[0] }\n}\nfn main() {}\n";
        let out = rename_module_collisions(src).unwrap_or_else(|| src.to_string());
        assert!(!out.contains("utils__N"), "{out}");
        assert!(out.contains("const N: usize>(_s: &[u8]) -> [W; N]"), "{out}");
        assert!(out.contains("pub fn first<const N: usize>(a: [u8; N])"), "{out}");
    }

    /// `const unsafe fn` (and `const async fn`, `const extern "C" fn`) is a fn, not a constant named `unsafe`: libc
    /// 0.2 has `pub const unsafe fn` in several modules, and taking `unsafe` for a colliding name renamed the
    /// `unsafe` of `unsafe impl Send for DIR {}` to `a__unsafe`
    #[test]
    fn const_unsafe_fn_is_not_a_constant() {
        let src = "mod a {\n    pub struct DIR;\n    pub const unsafe fn f() -> u8 { 1 }\n    unsafe impl Send for DIR {}\n}\nmod b {\n    pub struct DIR;\n    pub const unsafe fn f() -> u8 { 2 }\n    unsafe impl Send for DIR {}\n}\nfn main() {}\n";
        let out = rename_module_collisions(src).unwrap_or_else(|| src.to_string());
        assert!(!out.contains("__unsafe"), "{out}");
        assert!(out.contains("unsafe impl Send for a__DIR") && out.contains("unsafe impl Send for b__DIR"), "{out}");
    }

    /// An attribute's names and a macro's are no items (#251): typenum's `#[derive(Eq, ..)]` beside the `pub type Eq<A,
    /// B>` of `pub use crate::operator_aliases::*;` became `typenum__operator_aliases__Eq`. Nor do `#[repr(C)]` beside a
    /// `struct C`, `#[default]` beside a `fn default` and `write!` beside a `fn write` change; a macro's arguments and the
    /// alias in a type's place are still renamed
    #[test]
    fn attributes_and_macro_names_are_not_items() {
        let src = "mod typenum {\n    pub mod operator_aliases {\n        pub type Eq<A, B> = (A, B);\n        pub struct C;\n        pub fn write() {}\n        pub fn default() {}\n    }\n    pub use crate::operator_aliases::*;\n    #[derive(Eq, PartialEq, Clone, core::cmp::Ord, Debug, Default)]\n    #[repr(C)]\n    pub struct Greater;\n    #[derive(Default)]\n    pub enum E { #[default] A }\n    pub fn f(out: &mut String, e: Eq<u8, u16>) {\n        write!(out, \"{}\", default()).unwrap();\n        write![out, \"{:?}\", e].unwrap();\n    }\n}\nfn main() {}\n";
        let out = rename_module_collisions_in(src, &["typenum".to_string()]).unwrap();
        assert!(out.contains("#[derive(Eq, PartialEq, Clone, core::cmp::Ord, Debug, Default)]\n    #[repr(C)]\n    pub struct typenum__Greater;"), "{out}");
        assert!(out.contains("pub enum typenum__E { #[default] A }"), "{out}");
        assert!(out.contains("e: typenum__operator_aliases__Eq<u8, u16>)"), "{out}");
        assert!(out.contains("{\n        write!(out, \"{}\", typenum__operator_aliases__default()).unwrap();\n        write![out, \"{:?}\", e].unwrap();"), "{out}");
    }

    /// Review of #272: only a macro's name is in the macro namespace; the modules of its path are a path's. Two fn
    /// bodies' `mod m`, renamed for hoisting, take the `m` of their `m::say!(..)` with them
    #[test]
    fn a_macro_path_keeps_its_hoisted_module() {
        let src = "fn a() {\n    mod m {\n        pub use std::println as say;\n    }\n    m::say!(\"hello a\");\n}\nfn b() {\n    mod m {\n        pub use std::println as say;\n    }\n    m::say!(\"hello b\");\n}\nfn main() {}\n";
        let out = hoist_nested_items(src).unwrap();
        for (call, name) in [("hello a", "m__h0"), ("hello b", "m__h1")] {
            assert!(out.contains(&format!("    {name}::say!(\"{call}\");")), "{out}");
            assert!(out.contains(&format!("mod {name} {{")), "{out}");
        }
    }

    /// Review of #272: a macro's path names its module as any path does, and the macro's name is what the module
    /// imports as a macro: std's `println` that m brings as `say`, not m's `fn say` beside it (a value, renamed for
    /// n's `fn say`)
    #[test]
    fn a_macro_path_names_the_module_s_macro() {
        let src = "mod m {\n    pub use std::println as say;\n    pub fn say() -> u8 { 1 }\n}\nmod n {\n    pub fn say() -> u8 { 2 }\n}\nfn main() {\n    m::say!(\"{} {}\", m::say(), n::say());\n    crate::m::say![\"x\"];\n}\n";
        let out = rename_module_collisions_in(src, &[]).unwrap();
        assert!(out.contains("    std::println!(\"{} {}\", m__say(), n__say());\n    std::println![\"x\"];"), "{out}");
        // (in a crate module, through `crate::` and `self::` too)
        let src = "mod d {\n    pub mod m {\n        pub use std::println as say;\n        pub fn say() {}\n    }\n    pub fn f() {\n        m::say!(\"x\");\n        crate::m::say!(\"y\");\n        self::m::say! { \"z\" }\n    }\n}\nfn main() {}\n";
        let out = ren(src);
        assert!(out.contains("        std::println!(\"x\");\n        std::println!(\"y\");\n        std::println! { \"z\" }"), "{out}");
    }

    #[test]
    fn formatting_and_comments_do_not_change_hash() {
        let a = hashes("fn f(x: u32) -> u32 { x + 1 }\nfn g() {}");
        let b = hashes("// hi\nfn f(x: u32)->u32{\n  /* c */ x+1\n}\n\nfn g() { }");
        assert_eq!(a, b);
    }

    #[test]
    fn edits_change_only_that_item() {
        let a = hashes("fn f() { 1; }\nfn g() { 2; }");
        let b = hashes("fn f() { 1; }\nfn g() { 3; }");
        assert_eq!(a[0], b[0]);
        assert_ne!(a[1], b[1]);
    }

    #[test]
    fn impl_members_are_separate_items() {
        let src = "struct S { x: i64 }\nimpl S {\n    fn a(&self) -> i64 { self.x }\n    fn b() {}\n}\nfn main() {}";
        let it = split(src).unwrap();
        let kinds: Vec<_> = it.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ItemKind::Normal,
                ItemKind::ImplHeader,
                ItemKind::ImplMember(1),
                ItemKind::ImplMember(1),
                ItemKind::Normal
            ]
        );
        assert_eq!(
            &src[it[2].start..it[2].end],
            "fn a(&self) -> i64 { self.x }"
        );
        assert_eq!(it[2].line, 3);
        // メソッド b だけ編集すると a のハッシュは変わらない
        let it2 = split(&src.replace("fn b() {}", "fn b() { 1; }")).unwrap();
        assert_eq!(it[2].hash, it2[2].hash);
        assert_ne!(it[3].hash, it2[3].hash);
        // self 型が変わるとメンバ全部が変わる
        let it3 = split(&src.replace("impl S", "impl T")).unwrap();
        assert_ne!(it[2].hash, it3[2].hash);
    }

    #[test]
    fn use_and_const_items_end_at_semicolon() {
        let src = "use core::cmp::{max, min};\nconst P: Pt = Pt { x: 1 };\nconst fn f() -> u8 { 1 }\nfn main() {}";
        let it = split(src).unwrap();
        let texts: Vec<&str> = it.iter().map(|i| &src[i.start..i.end]).collect();
        assert_eq!(
            texts,
            vec![
                "use core::cmp::{max, min};",
                "const P: Pt = Pt { x: 1 };",
                "const fn f() -> u8 { 1 }",
                "fn main() {}"
            ]
        );
    }

    #[test]
    fn jointness_matters() {
        assert_ne!(hashes("fn f() { a && b; }"), hashes("fn f() { a & &b; }"));
    }

    #[test]
    fn strings_chars_lifetimes() {
        let it = split(
            "fn f() { let s = \"}\"; let c = '}'; let b = b'}'; let r = r#\"}\"#; }\nfn g<'a>() {}",
        )
        .unwrap();
        assert_eq!(it.len(), 2);
    }
}

/// 1 ファイルの中のモジュール `mod name { .. }` を平らにする: `mod name {` (と前の `pub` / 属性) と
/// 対応する `}` を空白で塗りつぶし、中のアイテムをトップレベルのアイテムにする (バイト位置と行は変えない)。
/// `#[cfg(test)] mod tests { .. }` はまるごと消す。モジュールのパス (`geo::f`) は名前解決が先頭を読み飛ばす。
/// モジュールがなければ None
/// 簡単な字句解析: (種類, 開始, 終了)。0 = 識別子, 1 = 記号 1 文字, 2 = リテラル (コメントは飛ばす)
/// Edition 2021 reserves `prefix#`, `prefix"..."` and `prefix'..'` for every identifier but the literal prefixes (`b`,
/// `r`, `br`, `c`, `cr`): `foo#bar` is no longer the three tokens `foo # bar`. The first such one, as
/// `line:col: prefix `foo` is unknown`. A look at the bytes first: tokenized only when an identifier character is
/// right before one of the three
pub fn unknown_prefix(src: &str) -> Option<String> {
    // (it runs on every file a build reads: no tokens, a jump from one of `/` `"` `'` `#` to the next, as only they
    // start a comment or literal or can follow a prefix. A string's or char's closing quote is stepped over with it,
    // so a quote looked at is an opening one, and the identifier right before it its prefix)
    let b = src.as_bytes();
    let n = b.len();
    // (the identifier ending right before i: its start, when it is one (not a number's suffix `1u8`, a lifetime
    // `'a`, a macro's fragment `$x`)
    let prefix_at = |i: usize| -> Option<usize> {
        let mut a = i;
        while a > 0 && is_ident_cont(b[a - 1]) {
            a -= 1;
        }
        // (and not a raw name `r#let`, whose `#` is not a prefix's)
        let raw = a >= 2 && b[a - 1] == b'#' && b[a - 2] == b'r' && (a == 2 || !is_ident_cont(b[a - 3]));
        // (a literal's suffix `"x"tag` / `'c'tag` is no prefix either; a lifetime's name is looked at with its quote. A
        // macro's `$x#y` is one: rustc's lexer reads `$` alone)
        (a < i && is_ident_start(b[a]) && !raw && !(a > 0 && matches!(b[a - 1], b'\'' | b'"'))).then_some(a)
    };
    let skip_str = |mut i: usize| {
        while i < n && b[i] != b'"' {
            i += if b[i] == b'\\' { 2 } else { 1 };
        }
        (i + 1).min(n)
    };
    // (from a raw string's first `#` or `"` at i: past its end `"#..`)
    let skip_raw = |i: usize| {
        let mut j = i;
        while j < n && b[j] == b'#' {
            j += 1;
        }
        let h = j - i;
        let mut k = j + 1;
        while k < n && !(b[k] == b'"' && k + h < n && b[k + 1..=k + h].iter().all(|&x| x == b'#')) {
            k += 1;
        }
        (k + 1 + h).min(n)
    };
    // (a char literal at the quote i, or a lifetime's quote: past it)
    let skip_quote = |i: usize| {
        if b.get(i + 1) == Some(&b'\\') {
            let mut j = i + 3;
            while j < n && b[j] != b'\'' {
                j += 1;
            }
            return (j + 1).min(n);
        }
        let l = utf8_len(b.get(i + 1).copied().unwrap_or(0));
        if b.get(i + 1 + l) == Some(&b'\'') { i + 2 + l } else { i + 1 }
    };
    let mut i = 0;
    while i < n {
        i = next_special(b, i);
        if i >= n {
            break;
        }
        let c = b[i];
        if c == b'/' {
            if b.get(i + 1) == Some(&b'/') {
                i = b[i..].iter().position(|&c| c == b'\n').map_or(n, |k| i + k);
            } else if b.get(i + 1) == Some(&b'*') {
                let mut d = 0;
                while i < n {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        d += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        d -= 1;
                        i += 2;
                        if d == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            } else {
                i += 1;
            }
            continue;
        }
        let w = prefix_at(i).map(|a| (a, &src[a..i]));
        match (c, w) {
            (b'"', None | Some((_, "b" | "c"))) => i = skip_str(i + 1),
            (b'"', Some((_, "r" | "br" | "cr"))) => i = skip_raw(i),
            (b'\'', None) | (b'\'', Some((_, "b"))) => {
                let j = skip_quote(i);
                if j != i + 1 {
                    i = j;
                    continue;
                }
                // (a lifetime: `'a#` is a reserved prefix too, but for the raw lifetime `'r#a`)
                let mut e = j;
                while e < n && is_ident_cont(b[e]) {
                    e += 1;
                }
                if e > j && e < n && b[e] == b'#' && &src[j..e] != "r" {
                    return Some(prefix_error(b, j, &src[j..e]));
                }
                i = e;
            }
            (b'#', None) => i += 1,
            (b'#', Some((_, "r" | "br" | "cr"))) => {
                let mut j = i;
                while j < n && b[j] == b'#' {
                    j += 1;
                }
                if b.get(j) == Some(&b'"') {
                    i = skip_raw(i);
                } else if w.is_some_and(|(_, w)| w == "r") && j == i + 1 {
                    // (a raw name `r#let`: past it)
                    i = j;
                    while i < n && is_ident_cont(b[i]) {
                        i += 1;
                    }
                } else {
                    let (a, w) = w.unwrap();
                    return Some(prefix_error(b, a, w));
                }
            }
            (_, Some((a, w))) => return Some(prefix_error(b, a, w)),
            _ => i += 1,
        }
    }
    None
}

/// The first of `/` `"` `'` `#` at or after i (b.len() if none): 8 bytes at a time (a byte equal to c makes a zero
/// byte of `x ^ c..c`, found by the carry trick)
fn next_special(b: &[u8], mut i: usize) -> usize {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let has = |x: u64, c: u8| {
        let y = x ^ (LO * c as u64);
        y.wrapping_sub(LO) & !y & HI
    };
    while i + 8 <= b.len() {
        let x = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let m = has(x, b'/') | has(x, b'"') | has(x, b'\'') | has(x, b'#');
        if m != 0 {
            break;
        }
        i += 8;
    }
    while i < b.len() && !matches!(b[i], b'/' | b'"' | b'\'' | b'#') {
        i += 1;
    }
    i
}

fn prefix_error(b: &[u8], a: usize, w: &str) -> String {
    let line = b[..a].iter().filter(|&&c| c == b'\n').count() + 1;
    let col = a - b[..a].iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1) + 1;
    format!("{line}:{col}: prefix `{w}` is unknown")
}

pub fn tokenize(s: &[u8]) -> Vec<(u8, usize, usize)> {
    let n = s.len();
    // (種類, 開始, 終了): 0 = 識別子, 1 = 記号 1 文字, 2 = リテラル
    let mut toks: Vec<(u8, usize, usize)> = vec![];
    let mut i = 0;
    while i < n {
        let c = s[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < n && s[i + 1] == b'/' {
            while i < n && s[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < n && s[i + 1] == b'*' {
            let mut d = 0;
            while i < n {
                if s[i] == b'/' && i + 1 < n && s[i + 1] == b'*' {
                    d += 1;
                    i += 2;
                } else if s[i] == b'*' && i + 1 < n && s[i + 1] == b'/' {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if let Some(end) = string_lit_at(s, i) {
            toks.push((2, i, end));
            i = end;
            continue;
        }
        if is_ident_start(c) {
            let st = i;
            while i < n && is_ident_cont(s[i]) {
                i += 1;
            }
            toks.push((0, st, i));
            continue;
        }
        if c == b'\'' {
            // 文字リテラル (ライフタイムは識別子の続きとして読む)
            let st = i;
            if i + 1 < n && s[i + 1] == b'\\' {
                i += 2;
                while i < n && s[i] != b'\'' {
                    i += 1;
                }
                i += 1;
                toks.push((2, st, i.min(n)));
                continue;
            }
            let l = utf8_len(s.get(i + 1).copied().unwrap_or(0));
            if i + 1 + l < n && s[i + 1 + l] == b'\'' {
                i += 2 + l;
                toks.push((2, st, i));
                continue;
            }
            i += 1;
            continue;
        }
        toks.push((1, i, i + 1));
        i += 1;
    }
    toks
}

/// The name a prelude item defines, from its identifiers (`idents`, ranges of src): a fn's, struct's, enum's, static's or
/// const's (an associated const's too), or a type alias's (`normal`: an item not in an impl or trait), without the
/// `__core_` of a core-only item. A member of an impl of a radix or exponent fmt trait (`impl LowerHex for u8`; header:
/// its impl's identifiers) is looked up by the trait's name, which a program that uses the impls names, not by `fmt`,
/// which every program that formats needs. (Here, not in prelude.rs: build.rs works the names out ahead of time)
pub fn prelude_item_name<'a>(src: &'a str, idents: &[(u32, u32)], normal: bool, header: Option<&[(u32, u32)]>) -> Option<&'a str> {
    if let Some(h) = header {
        let mut hw = h.iter().map(|&(a, b)| &src[a as usize..b as usize]);
        if let (Some("impl"), Some(t), Some("for")) = (hw.next(), hw.next(), hw.next()) {
            if matches!(t, "Binary" | "Octal" | "LowerHex" | "UpperHex" | "LowerExp" | "UpperExp") {
                return Some(t);
            }
        }
    }
    let mut ids = idents.iter().map(|&(a, b)| &src[a as usize..b as usize]);
    ids.find(|w| matches!(*w, "fn" | "struct" | "enum" | "static" | "const") || (normal && *w == "type"))?;
    let mut n = ids.next()?;
    // (`const fn f`: f; `static mut X`: X, not `mut`, which almost every program names)
    if n == "fn" || n == "mut" {
        n = ids.next()?;
    }
    Some(n.strip_prefix("__core_").unwrap_or(n))
}

/// What a proc macro's expansion starts with (crate::pm): a comment, so the preprocessing knows that a line holds one
pub const PM_MARK: &str = "/*zust:pm*/";

/// Whether this process expanded a proc macro (crate::pm::expand): only then can a text hold PM_MARK
pub static PM_EXPANDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 関数の本体の中に書いたアイテム (`fn helper() {..}` / `struct` / `enum` / `const` / `use`) をファイルの末尾へ移す。
/// 元の場所は空白にして (改行は残す)、後ろの行番号を変えない
pub fn hoist_nested_items(src: &str) -> Option<String> {
    let (body, moves) = hoist_nested_items_parts(src)?;
    if moves.is_empty() {
        return Some(body);
    }
    let mut out = body.into_bytes();
    for (_, t) in &moves {
        out.push(b'\n');
        out.extend_from_slice(t.as_bytes());
    }
    out.push(b'\n');
    String::from_utf8(out).ok()
}

/// What an item that hoist_parts_at may move starts with, by its first letter: the qualified fns, impls and traits
/// too (`unsafe fn`, `async fn` / `async unsafe fn`, `unsafe impl`)
fn hoist_kws(c: u8) -> &'static [&'static str] {
    match c {
        b'f' => &["fn "],
        b's' => &["struct ", "static "],
        b'e' => &["enum ", "extern "],
        b'u' => &["use ", "union ", "unsafe extern ", "unsafe fn ", "unsafe impl", "unsafe trait "],
        b'c' => &["const "],
        b'm' => &["mod "],
        b't' => &["trait ", "type "],
        b'i' => &["impl"],
        b'p' => &["pub fn ", "pub struct ", "pub enum ", "pub const ", "pub static ", "pub trait ", "pub type ", "pub(crate) "],
        b'a' => &["async "],
        _ => &[],
    }
}

/// An item keyword right after a brace or a `;` on a line (`f({ fn inner() {} })`), which hoist_parts_at's quick
/// check looks for besides indented ones. A text check: it also answers yes for a comment, a string or a one-line impl
/// (`impl T for u8 { fn f() {} }`), which only costs the tokenizing that check would skip
pub fn has_inline_item(src: &str) -> bool {
    // (the keywords hoist_parts_at looks for)
    let b = src.as_bytes();
    let at = |i: usize| -> bool {
        // (most of these end a line: a look at the next byte decides them)
        let mut j = i + 1;
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        if j >= b.len() || !(b[j].is_ascii_lowercase() || b[j] == b'#') {
            return false;
        }
        let mut t = &src[j..];
        // (after attributes on the same line too: `{ #[inline] fn f() {} }`)
        while let Some(rest) = t.strip_prefix("#[") {
            let Some(end) = rest.find(']') else { break };
            // (an attribute with a string or brackets in it (`#[doc = "]"]`) is not followed: taken as an item, the
            // full preparation decides)
            if rest[..end].contains(['"', '[', '\'']) {
                return true;
            }
            t = rest[end + 1..].trim_start_matches([' ', '\t']);
        }
        t.as_bytes().first().is_some_and(|&c| hoist_kws(c).iter().any(|k| t.starts_with(k)))
    };
    // (each of the three looked for alone: str::find of an ASCII char is a memchr)
    for c in ['{', ';', '}'] {
        let mut from = 0;
        while let Some(k) = src[from..].find(c) {
            if at(from + k) {
                return true;
            }
            from += k + 1;
        }
    }
    false
}

/// hoist_nested_items を、(元の場所を空白にした本文 (長さと改行は入力と同じ。名前を変えたときは改行だけ同じ),
/// 移したアイテム (本文での開始位置, テキスト) の列) で返す。末尾に `\n` + テキストを順に並べ、最後に `\n` を付けたものが
/// hoist_nested_items の結果 (列が空なら本文だけ)
pub fn hoist_nested_items_parts(src: &str) -> Option<(String, Vec<(usize, String)>)> {
    hoist_parts_at(src, 0)
}

/// (namespaces of a hoisted item's name, for its renames)
const HOIST_NS_VALUE: u8 = 0;
const HOIST_NS_TYPE: u8 = 1;
const HOIST_NS_BOTH: u8 = 2;

/// Where a moved item's text begins: its first doc comment (`/// ..` / `/** .. */`) between the token before it (lo)
/// and its first token (from), or that token. Between two tokens there are only whitespace and comments
#[inline(never)]
fn doc_start(s: &[u8], lo: usize, from: usize) -> usize {
    let mut i = lo;
    let mut first_doc = None;
    while i < from {
        if s[i..].starts_with(b"//") {
            if s[i..].starts_with(b"///") && !s[i..].starts_with(b"////") {
                first_doc.get_or_insert(i);
            }
            while i < from && s[i] != b'\n' {
                i += 1;
            }
        } else if s[i..].starts_with(b"/*") {
            if s[i..].starts_with(b"/**") && !s[i..].starts_with(b"/**/") {
                first_doc.get_or_insert(i);
            }
            let mut d = 0i32;
            while i + 1 < from {
                if s[i] == b'/' && s[i + 1] == b'*' {
                    d += 1;
                    i += 2;
                } else if s[i] == b'*' && s[i + 1] == b'/' {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    first_doc.unwrap_or(from)
}

/// Whether the `{` at token k, after a `=`, opens an anonymous const's body: `const _: () = {` (looked for at the
/// `{`, which are few, not at each `const`). Its type is a few tokens: not past a `;` or a brace
#[inline(never)]
fn anon_const_head(s: &[u8], toks: &[(u8, usize, usize)], k: usize) -> bool {
    let is_sym = |j: usize, c: u8| toks[j].0 == 1 && s[toks[j].1] == c;
    let ident = |j: usize, w: &[u8]| toks[j].0 == 0 && &s[toks[j].1..toks[j].2] == w;
    let mut j = k - 2;
    while j >= 1 && k - j < 16 {
        if is_sym(j, b';') || is_sym(j, b'{') || is_sym(j, b'}') {
            return false;
        }
        if ident(j, b"_") && ident(j - 1, b"const") && is_sym(j + 1, b':') {
            return true;
        }
        j -= 1;
    }
    false
}

/// The body of an anonymous const that opens at token k (`const _: () = {`), when it is on one line (a macro wrote
/// it): the names of the types it defines at its top level, which stay in it. None when it is not on one line (read
/// where it is, as before); Some(None) when it is skipped whole, as before this branch: an item that would go out names
/// a type, const or static that stays (`const _: () = { type Local = u32; fn helper(_: Local) {} };` would lose `Local`)
#[inline(never)]
fn anon_body_types<'a>(s: &'a [u8], toks: &[(u8, usize, usize)], k: usize) -> Option<Option<Vec<&'a [u8]>>> {
    let tok = |k: usize| -> &'a [u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && s[toks[k].1] == c;
    let ident = |k: usize| toks[k].0 == 0;
    // (an item's first word: after the body's `{`, a `;`, a `}`, an attribute's `]` or `pub`; not a const generic
    // parameter, `fn invoke<R, const UNWIND_SAFE: bool>`, whose `<` is no bracket here)
    let item_start = |j: usize| j == k + 1 || is_sym(j - 1, b';') || is_sym(j - 1, b'}') || is_sym(j - 1, b']') || is_sym(j - 1, b')') || (ident(j - 1) && tok(j - 1) == b"pub");
    let mut names: Vec<&[u8]> = vec![];
    // (and the consts and statics, which stay too)
    let mut values: Vec<&[u8]> = vec![];
    let mut d = 0i32;
    let mut j = k + 1;
    while j < toks.len() {
        if is_sym(j, b'{') || is_sym(j, b'(') || is_sym(j, b'[') {
            d += 1;
        } else if is_sym(j, b'}') || is_sym(j, b')') || is_sym(j, b']') {
            if d == 0 {
                break;
            }
            d -= 1;
        } else if d == 0 && ident(j) && matches!(tok(j), b"struct" | b"enum" | b"union" | b"type") && j + 1 < toks.len() && ident(j + 1) && item_start(j) {
            names.push(tok(j + 1));
        } else if d == 0 && ident(j) && matches!(tok(j), b"const" | b"static") && j + 2 < toks.len() && item_start(j) {
            let n = if tok(j + 1) == b"mut" { j + 2 } else { j + 1 };
            if ident(n) && tok(n) != b"_" && is_sym(n + 1, b':') {
                values.push(tok(n));
            }
        }
        j += 1;
    }
    if j >= toks.len() || s[toks[k].1..toks[j].1].contains(&b'\n') {
        return None;
    }
    if names.is_empty() && values.is_empty() {
        return Some(Some(names));
    }
    // (its items at the top level, from their first word to their `;` or closing brace: one that goes out (a fn,
    // `use`, trait, extern block, or an impl for none of its types) must not name what stays)
    let mut i = k + 1;
    while i < j {
        if is_sym(i, b'#') || is_sym(i, b';') {
            i += 1;
            continue;
        }
        let start = i;
        let mut kind: Option<&[u8]> = None;
        let mut d = 0i32;
        let mut end = i;
        while end < j {
            if is_sym(end, b'{') || is_sym(end, b'(') || is_sym(end, b'[') {
                d += 1;
            } else if is_sym(end, b'}') || is_sym(end, b')') || is_sym(end, b']') {
                d -= 1;
                if d == 0 && is_sym(end, b'}') && kind.is_some_and(|k| k != b"const" && k != b"static" && k != b"type") {
                    break;
                }
            } else if d == 0 && is_sym(end, b';') {
                break;
            } else if d == 0 && kind.is_none() && ident(end) {
                let w = tok(end);
                kind = match w {
                    // (`const fn`, `unsafe extern "C" fn`: the fn)
                    b"const" | b"unsafe" | b"extern" | b"async" if end + 1 < j && (ident(end + 1) && tok(end + 1) != b"_" && !is_sym(end + 2, b':') || toks[end + 1].0 == 2) && w != b"const" || (w == b"const" && end + 1 < j && matches!(tok(end + 1), b"fn" | b"unsafe" | b"async" | b"extern")) => None,
                    b"fn" | b"use" | b"trait" | b"impl" | b"mod" | b"struct" | b"enum" | b"union" | b"type" | b"const" | b"static" | b"extern" => Some(w),
                    _ => None,
                };
            }
            end += 1;
        }
        let local_impl = kind == Some(b"impl") && {
            let mut h = start;
            while h < end && !is_sym(h, b'{') {
                h += 1;
            }
            (start..h).any(|q| ident(q) && names.contains(&tok(q)))
        };
        let goes_out = matches!(kind, Some(b"fn" | b"use" | b"trait" | b"impl" | b"mod" | b"extern")) && !local_impl;
        if goes_out && (start..end.min(j)).any(|q| ident(q) && (names.contains(&tok(q)) || values.contains(&tok(q)))) {
            return Some(None);
        }
        i = end + 1;
    }
    Some(Some(names))
}

/// The token of the `}` that closes the `{` at token k (the last token when it is not closed)
fn brace_close(s: &[u8], toks: &[(u8, usize, usize)], k: usize) -> usize {
    let mut d = 0i32;
    for j in k..toks.len() {
        if toks[j].0 == 1 && s[toks[j].1] == b'{' {
            d += 1;
        } else if toks[j].0 == 1 && s[toks[j].1] == b'}' {
            d -= 1;
            if d == 0 {
                return j;
            }
        }
    }
    toks.len().saturating_sub(1)
}

/// What is left of src's delimiters (`(` / `[` / `{`, by tokens: not those in strings, chars or comments) once those
/// that match each other are taken away: the closers that close none of its own, in order, then the openers that
/// none of its own close, in order. None when one closes another kind (`(]`). A text's delimiters match when what is
/// left of its parts', in order, matches (prep_inc: per chunk, where a chunk can start inside an `impl` block);
/// hoist_parts_at moves nothing out of a source whose do not
pub fn delims_left(src: &str) -> Option<Vec<u8>> {
    let s = src.as_bytes();
    let mut closers: Vec<u8> = vec![];
    let mut open: Vec<u8> = vec![];
    for t in tokenize(s) {
        if t.0 != 1 {
            continue;
        }
        match s[t.1] {
            c @ (b'(' | b'[' | b'{') => open.push(c),
            c @ (b')' | b']' | b'}') => match open.pop() {
                Some(o) if o == opener_of(c) => {}
                Some(_) => return None,
                None => closers.push(c),
            },
            _ => {}
        }
    }
    closers.extend(open);
    Some(closers)
}

/// The opening delimiter a closing one closes
pub fn opener_of(c: u8) -> u8 {
    match c {
        b')' => b'(',
        b']' => b'[',
        _ => b'{',
    }
}

/// depth: 名前を変えてやり直した回数 (壊れたソースで名前の変更が止まらないときは 8 回で止める)
fn hoist_parts_at(src: &str, redo: u32) -> Option<(String, Vec<(usize, String)>)> {
    // (速く飛ばす: 字下げした行頭の `fn ` などがなければ入れ子のアイテムはない)
    // (by the first letter: most lines start with none of them, and a line is tried against that letter's only)
    let kws = hoist_kws;
    let indented = src.split('\n').any(|l| {
        let t = l.trim_start();
        t.len() < l.len() && t.as_bytes().first().is_some_and(|&c| kws(c).iter().any(|k| t.starts_with(k)))
    });
    // (a proc macro's output is one line, marked (PM_MARK): the items in its bodies, `{#[link(..)] extern "C" {..}`,
    // are not at a line's start. Looked for only once this process expanded one)
    // (nor an anonymous const a `macro_rules!` expansion wrote on one line, `const _: () = { impl Tr for u8 {..} };`)
    // (nor an item after a `{` / `;` / `}` on its line, `fn main() { fn g() {} g() }`: looked for here too, so that
    // whether it is moved does not depend on the rest of the source having an indented item (prep's incremental path
    // preprocesses a source of the changed chunks and the others' skeletons))
    if !indented
        && !(PM_EXPANDED.load(std::sync::atomic::Ordering::Relaxed) && src.contains(PM_MARK))
        && !src.contains("const _")
        && !has_inline_item(src)
    {
        return None;
    }
    let s = src.as_bytes();
    let toks = tokenize(s);
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    // (記号のトークンは 1 byte)
    let is_sym = |k: usize, c: &[u8]| toks[k].0 == 1 && s[toks[k].1] == c[0];
    // 各位置の波括弧の深さと、関数の本体の深さのスタック
    let mut depth = 0i32;
    // (a `}` with no `{` to close)
    let mut unbalanced = false;
    let mut fn_bodies: Vec<(i32, usize)> = vec![];
    let mut pending_fn = false;
    // (the bodies of `const _: () = { .. };`, what derives and `#[wasm_bindgen]` wrap impls in: their items but their
    // consts and statics go out, as a fn body's do. Each one's `{` (token), and the types it defines, which stay in it:
    // an impl for one stays too)
    let mut anons: Vec<(usize, Vec<&[u8]>)> = vec![];
    // (a top-level `const` / `static` whose initializer is a block, `const F: fn() = { fn broken() {..} broken };`: the
    // block's items are hoisted as a fn body's are. Set at the item's keyword, until its `;`)
    let mut init_item = false;
    // (the nesting of `(` / `[`: a `;` in them (`[u8; 4]`, `[(); 1]`) ends no item)
    let mut nest = 0i32;
    let mut moves: Vec<(usize, usize)> = vec![];
    // 入れ子のアイテムの名前と、それを含む関数の本体 (開き括弧のトークン位置)。閉じた本体の (開き, 閉じ)
    // (and its namespace: HOIST_NS_VALUE (fn / static / const), HOIST_NS_TYPE (a braced struct, enum, trait, ..), or HOIST_NS_BOTH (a
    // tuple or unit struct, and the rest))
    let mut nested_names: Vec<(Vec<u8>, usize, u8)> = vec![];
    let mut bodies: Vec<(usize, usize)> = vec![];
    // (an item's scope is its innermost block, `{ struct Bar; } { struct Bar; }` two: the `{` before its statement,
    // found back from it, as items in bodies are few)
    let block_of = |k: usize| -> usize {
        let mut d = 0u32;
        let mut j = k;
        while j > 0 {
            j -= 1;
            if toks[j].0 == 1 {
                match s[toks[j].1] {
                    b'}' => d += 1,
                    b'{' if d == 0 => return j,
                    b'{' => d -= 1,
                    _ => {}
                }
            }
        }
        0
    };
    // トップレベルのアイテムの名前 (入れ子のアイテムを外に出すとぶつかるもの)
    let mut top_names: Vec<Vec<u8>> = vec![];
    let mut k = 0;
    while k < toks.len() {
        let t = tok(k);
        if toks[k].0 == 0 {
            if depth == 0 && matches!(t, b"const" | b"static") && k + 1 < toks.len() && !matches!(tok(k + 1), b"fn" | b"unsafe" | b"extern" | b"async") {
                init_item = true;
            }
            if depth == 0 && matches!(t, b"fn" | b"struct" | b"enum" | b"const" | b"static" | b"union" | b"trait" | b"type") && k + 1 < toks.len() {
                let nk = if t == b"static" && tok(k + 1) == b"mut" { k + 2 } else { k + 1 };
                if nk < toks.len() && toks[nk].0 == 0 {
                    top_names.push(tok(nk).to_vec());
                }
            }
            let in_body = fn_bodies.last().is_some_and(|&(d, _)| depth > d);
            // (`pub fn` / `pub(crate) struct` inside a body: what flattening a `mod` in a function leaves)
            // (most identifiers are not at the start of a statement in a body: decide that first, it is cheap)
            let stmt_start = in_body && (k == 0 || is_sym(k - 1, b"{") || is_sym(k - 1, b"}") || is_sym(k - 1, b";") || is_sym(k - 1, b"]"));
            if !stmt_start {
                if t == b"fn" && k + 1 < toks.len() && toks[k + 1].0 == 0 {
                    pending_fn = true;
                }
                k += 1;
                continue;
            }
            // (`unsafe fn` / `extern "C" fn` / `unsafe extern "C" fn` / `async fn`: the `fn`)
            let qual_fn = |mut j: usize| -> Option<usize> {
                let mut any = false;
                while j < toks.len() && toks[j].0 == 0 && matches!(tok(j), b"unsafe" | b"async" | b"extern") {
                    if tok(j) == b"extern" && j + 1 < toks.len() && toks[j + 1].0 == 2 {
                        j += 1;
                    }
                    j += 1;
                    any = true;
                }
                (any && j + 1 < toks.len() && toks[j].0 == 0 && tok(j) == b"fn" && toks[j + 1].0 == 0).then_some(j)
            };
            let ik = if t == b"pub" && k + 1 < toks.len() {
                let mut j = k + 1;
                if is_sym(j, b"(") {
                    while j < toks.len() && !is_sym(j, b")") {
                        j += 1;
                    }
                    j += 1;
                }
                if j < toks.len() && toks[j].0 == 0 && matches!(tok(j), b"fn" | b"struct" | b"enum" | b"const" | b"use" | b"static" | b"mod" | b"trait" | b"type" | b"union") {
                    j
                } else if let Some(f) = qual_fn(j) {
                    f
                } else {
                    k
                }
            } else if let Some(f) = matches!(t, b"unsafe" | b"async" | b"extern").then(|| qual_fn(k)).flatten() {
                f
            } else if t == b"unsafe" && k + 1 < toks.len() && matches!(tok(k + 1), b"impl" | b"trait") {
                // (`unsafe impl Tr for S { .. }` / `unsafe trait Tr { .. }`)
                k + 1
            } else if t == b"unsafe" && k + 1 < toks.len() && matches!(tok(k + 1), b"extern" | b"fn") {
                // (`unsafe extern "C" { .. }`, `unsafe fn f() { .. }`)
                k + 1
            } else if matches!(t, b"async" | b"const" | b"unsafe" | b"safe" | b"extern") && {
                // (a fn's qualifiers, `async fn`, `const unsafe fn`, `extern "C" fn`: the item is the fn)
                let mut j = k;
                while j < toks.len() && (matches!(tok(j), b"async" | b"const" | b"unsafe" | b"safe" | b"extern") || (toks[j].0 == 2 && j > k && tok(j - 1) == b"extern")) {
                    j += 1;
                }
                j < toks.len() && j > k && tok(j) == b"fn"
            } {
                let mut j = k;
                while tok(j) != b"fn" {
                    j += 1;
                }
                j
            } else {
                k
            };
            let pt = t;
            let t = tok(ik);
            // (an extern block, `extern "C" { fn f(); }` (also with no ABI): the block goes out whole, its declarations
            // are not items of their own)
            let extern_block = t == b"extern"
                && ik + 1 < toks.len()
                && (is_sym(ik + 1, b"{") || (toks[ik + 1].0 == 2 && ik + 2 < toks.len() && is_sym(ik + 2, b"{")));
            // (`mod m { .. }` / `trait T { .. }` / `impl S { .. }` / `type A = ..;` / `union U { .. }` too; `union` is a
            // keyword only before a name)
            let item_kw = matches!(t, b"fn" | b"struct" | b"enum" | b"const" | b"use" | b"static" | b"mod" | b"trait" | b"impl" | b"type")
                || (t == b"union" && ik + 1 < toks.len() && toks[ik + 1].0 == 0)
                || extern_block;
            let impl_like = t == b"impl";
            // (in an anonymous const's body, only its impls, `use`s, fns, traits and extern blocks go out: its consts,
            // statics and types stay, what the const checks at compile time (`struct _Section([u8; _CHUNK_1_LEN])` of
            // `#[wasm_bindgen]`'s custom section, beside the const it names))
            let in_anon = fn_bodies.last().is_some_and(|&(_, open)| anons.last().is_some_and(|a| a.0 == open));
            let local_impl = || {
                let names = anons.last().map(|a| a.1.as_slice()).unwrap_or(&[]);
                let mut j = ik + 1;
                while j < toks.len() && !is_sym(j, b"{") && !is_sym(j, b";") {
                    if toks[j].0 == 0 && names.contains(&tok(j)) {
                        return true;
                    }
                    j += 1;
                }
                false
            };
            let anon_keeps = in_anon && !extern_block && (!matches!(t, b"impl" | b"use" | b"fn" | b"trait") || (t == b"impl" && local_impl()));
            // (an anonymous const that stays, in another one's body: its own body is one too)
            // (its `{` is found as any anonymous const's is)
            if anon_keeps && t == b"const" && ik + 1 < toks.len() && tok(ik + 1) == b"_" {
            } else if anon_keeps && item_kw && ik + 1 < toks.len() && (toks[ik + 1].0 == 0 || (impl_like && is_sym(ik + 1, b"<"))) {
                // (another item that stays, `impl Section { fn len(&self) .. }`, `impl<T> Local<T> { .. }`: past its end,
                // whose fns are its own)
                let semi_only = matches!(t, b"const" | b"static" | b"type" | b"use");
                let mut j = ik + 1;
                let mut d = 0i32;
                while j < toks.len() {
                    if is_sym(j, b"(") || is_sym(j, b"[") || (semi_only && is_sym(j, b"{")) {
                        d += 1;
                    } else if is_sym(j, b")") || is_sym(j, b"]") || (semi_only && is_sym(j, b"}")) {
                        d -= 1;
                    } else if d == 0 && is_sym(j, b";") {
                        break;
                    } else if d == 0 && is_sym(j, b"{") {
                        let mut e = 0i32;
                        while j < toks.len() {
                            if is_sym(j, b"{") {
                                e += 1;
                            } else if is_sym(j, b"}") {
                                e -= 1;
                                if e == 0 {
                                    break;
                                }
                            }
                            j += 1;
                        }
                        break;
                    }
                    j += 1;
                }
                k = j + 1;
                continue;
            }
            // (`const` の後ろに `fn` や `|` (const クロージャ) が来るものは別扱い: `const fn` は fn で見る)
            if in_body && stmt_start && item_kw && !anon_keeps && ik + 1 < toks.len() && (toks[ik + 1].0 == 0 || (impl_like && is_sym(ik + 1, b"<")) || extern_block) {
                // アイテムの終わり: `{ .. }` (fn / struct / enum) か `;`
                let mut j = ik + 1;
                let mut d = 0i32;
                let mut end = None;
                // (`const X: S = S { a: 0 };` / `static` / `type` / `use` end at the `;`, after any braces)
                let semi_only = matches!(t, b"static" | b"type" | b"use") || (t == b"const" && !matches!(tok(ik + 1), b"fn" | b"unsafe" | b"extern" | b"async"));
                while j < toks.len() {
                    if toks[j].0 == 1 {
                        match tok(j) {
                            b"(" | b"[" => d += 1,
                            b")" | b"]" => d -= 1,
                            b"{" if semi_only => d += 1,
                            b"}" if semi_only => d -= 1,
                            b";" if d == 0 => {
                                end = Some(j);
                                break;
                            }
                            b"{" if d == 0 => {
                                // 対応する }
                                let mut e = 0i32;
                                let mut m = j;
                                while m < toks.len() {
                                    if is_sym(m, b"{") {
                                        e += 1;
                                    } else if is_sym(m, b"}") {
                                        e -= 1;
                                        if e == 0 {
                                            break;
                                        }
                                    }
                                    m += 1;
                                }
                                end = Some(m);
                                break;
                            }
                            _ => {}
                        }
                    }
                    j += 1;
                }
                let Some(e) = end else { return None };
                if e >= toks.len() {
                    return None;
                }
                // 前の `#[..]` も一緒に
                let mut st = k;
                while st >= 1 && is_sym(st - 1, b"]") {
                    let mut m = st - 1;
                    let mut d = 0;
                    loop {
                        if is_sym(m, b"]") {
                            d += 1;
                        } else if is_sym(m, b"[") {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        if m == 0 {
                            break;
                        }
                        m -= 1;
                    }
                    if m >= 1 && is_sym(m - 1, b"#") {
                        st = m - 1;
                    } else {
                        break;
                    }
                }
                // (and its doc comments before those, `/// ..` / `/** .. */`: left behind they would document nothing,
                // E0585. Between two tokens there are only whitespace and comments)
                let from = doc_start(s, if st > 0 { toks[st - 1].2 } else { 0 }, toks[st].1);
                moves.push((from, toks[e].2));
                let nk = if matches!(t, b"static") && tok(ik + 1) == b"mut" { ik + 2 } else { ik + 1 };
                if nk < toks.len() && toks[nk].0 == 0 && t != b"use" && t != b"impl" && !extern_block {
                    let ns = match t {
                        b"fn" | b"static" | b"const" => HOIST_NS_VALUE,
                        b"enum" | b"union" | b"trait" | b"type" | b"mod" => HOIST_NS_TYPE,
                        // (`struct S { .. }` names a type only; `struct S(..);` / `struct S;` a constructor too. The
                        // first `{` / `(` / `;` past its generics tells)
                        b"struct" => {
                            let mut d = 0i32;
                            let mut ns = HOIST_NS_BOTH;
                            // (a `(` in a `where` clause, `where F: Fn(u8) {`, is a bound's, not a tuple struct's)
                            let mut in_where = false;
                            for q in nk + 1..e.min(toks.len()) {
                                if toks[q].0 != 1 {
                                    in_where |= d <= 0 && tok(q) == b"where";
                                    continue;
                                }
                                match tok(q) {
                                    b"<" => d += 1,
                                    // (not the `>` of `->`, `F: Fn() -> ()`)
                                    b">" if !(q >= 1 && toks[q - 1].0 == 1 && tok(q - 1) == b"-") => d -= 1,
                                    b"{" if d <= 0 => {
                                        ns = HOIST_NS_TYPE;
                                        break;
                                    }
                                    b"(" if d <= 0 && !in_where => break,
                                    b";" if d <= 0 => break,
                                    _ => {}
                                }
                            }
                            ns
                        }
                        _ => HOIST_NS_BOTH,
                    };
                    nested_names.push((tok(nk).to_vec(), block_of(k), ns));
                }
                // (an extern block's functions: names of the block's function, renamed like its other items when
                // another function's block declares the name too; the declaration then keeps its symbol, link_name)
                if extern_block {
                    for q in ik + 1..e {
                        if toks[q].0 == 0 && tok(q) == b"fn" && q + 1 < e && toks[q + 1].0 == 0 {
                            nested_names.push((tok(q + 1).to_vec(), block_of(k), HOIST_NS_VALUE));
                        }
                    }
                }
                k = e + 1;
                continue;
            }
            let _ = pt;
            // (a fn item has a name: `fn() -> T` is a type)
            if t == b"fn" && ik + 1 < toks.len() && toks[ik + 1].0 == 0 {
                pending_fn = true;
            }
        } else if toks[k].0 == 1 {
            match t {
                b"{" => {
                    if pending_fn {
                        fn_bodies.push((depth, k));
                        pending_fn = false;
                    } else {
                        // (an anonymous const a macro wrote on one line (a proc macro's output, a `macro_rules!`
                        // expansion): its own rules, anon_body_types)
                        let anon = if k >= 5 && is_sym(k - 1, b"=") && anon_const_head(s, &toks, k) { anon_body_types(s, &toks, k) } else { None };
                        match anon {
                            Some(Some(names)) => {
                                fn_bodies.push((depth, k));
                                anons.push((k, names));
                            }
                            // (one skipped whole: past its closing brace, so that its items are not moved out of a
                            // body around it either, `const _: () = { const _: () = { type Local = u32; fn helper(_:
                            // Local) {} }; };`)
                            Some(None) => {
                                k = brace_close(s, &toks, k) + 1;
                                continue;
                            }
                            // (another top-level const's or static's initializer block, `const F: fn() = { fn broken()
                            // {..} broken };`, a hand-written anonymous const's too: its items go out as a fn body's do)
                            None => {
                                if init_item && depth == 0 && k >= 1 && is_sym(k - 1, b"=") {
                                    fn_bodies.push((depth, k));
                                }
                            }
                        }
                    }
                    depth += 1;
                }
                b"}" => {
                    depth -= 1;
                    if depth < 0 {
                        unbalanced = true;
                    }
                    if fn_bodies.last().is_some_and(|&(d, _)| d == depth) {
                        let (_, open) = fn_bodies.pop().unwrap();
                        if anons.last().is_some_and(|a| a.0 == open) {
                            anons.pop();
                        }
                        bodies.push((open, k));
                    }
                }
                b"(" | b"[" => nest += 1,
                b")" | b"]" => nest = (nest - 1).max(0),
                // `fn f();` (宣言だけ) は本体なし
                b";" if nest == 0 => {
                    pending_fn = false;
                    if depth == 0 {
                        init_item = false;
                    }
                }
                _ => {}
            }
        }
        k += 1;
    }
    // (nor a source whose braces do not match, `fn f() { fn g() {} pub f<`: the lexer reports where, at a line of the
    // source's own, which an item moved past its last line would move the error away from (#188); and the renames of
    // such a source's items, thousands of `fn`s in one unclosed body, could take seconds. prep_inc, which matches
    // the parens and brackets too, never reuses chunks of such a source)
    if unbalanced || depth != 0 {
        return None;
    }
    if moves.is_empty() {
        return None;
    }
    // (別々の関数の中の同じ名前のアイテム (`static TABLE`) は、外に出すとぶつかるので、
    //  その関数の本体の中でだけ `TABLE__h1` などに名前を変えてからやり直す)
    let mut renames: Vec<(usize, usize, Vec<u8>, Vec<u8>)> = vec![];
    let mut rename_ns: Vec<u8> = vec![];
    for (i, (n, b, ns)) in nested_names.iter().enumerate() {
        let dup = nested_names.iter().enumerate().any(|(j, (m, c, _))| j != i && m == n && c != b) || top_names.contains(n);
        if !dup {
            continue;
        }
        // (the block's `}`: a fn body's is known, another block's is found forward from its `{`)
        let close_of = |o: usize| -> Option<usize> {
            let mut d = 0u32;
            for j in o..toks.len() {
                if toks[j].0 == 1 {
                    match s[toks[j].1] {
                        b'{' => d += 1,
                        b'}' => {
                            d -= 1;
                            if d == 0 {
                                return Some(j);
                            }
                        }
                        _ => {}
                    }
                }
            }
            None
        };
        if let Some((open, close)) = bodies.iter().find(|x| x.0 == *b).copied().or_else(|| close_of(*b).map(|c| (*b, c))) {
            let mut nn = n.clone();
            nn.extend_from_slice(format!("__h{i}").as_bytes());
            renames.push((open, close, n.clone(), nn));
            rename_ns.push(*ns);
        }
    }
    if !renames.is_empty() {
        // (名前を変えるトークン: その関数の本体の中だけを見る。外側の本体が先に来るので、内側のものが後で上書きする)
        // (a local binding of a renamed name, `let m = || 7;`: it and its uses, from the end of its statement to the
        // end of its block, are the local's and keep the name. Its initializer still sees the item)
        let mut local_toks = vec![false; toks.len()];
        for k in 0..toks.len() {
            if !(toks[k].0 == 0 && tok(k) == b"let") {
                continue;
            }
            let nk = if k + 1 < toks.len() && tok(k + 1) == b"mut" { k + 2 } else { k + 1 };
            if nk >= toks.len() || toks[nk].0 != 0 || !renames.iter().any(|r| r.2.as_slice() == tok(nk)) {
                continue;
            }
            local_toks[nk] = true;
            // (the statement's `;`, then the block's `}`)
            let mut d = 0i32;
            let mut j = nk + 1;
            while j < toks.len() {
                if toks[j].0 == 1 {
                    match tok(j) {
                        b"(" | b"[" | b"{" => d += 1,
                        b")" | b"]" | b"}" => {
                            if d == 0 {
                                break;
                            }
                            d -= 1;
                        }
                        b";" if d == 0 => break,
                        _ => {}
                    }
                }
                j += 1;
            }
            if j >= toks.len() || tok(j) != b";" {
                continue;
            }
            let mut d = 0i32;
            let mut q = j + 1;
            while q < toks.len() {
                if toks[q].0 == 1 {
                    match tok(q) {
                        // (a block in it that declares a value of the name, `{ fn n() .. n() }`: the item there. A
                        // braced struct shadows no value)
                        b"{" if nested_names.iter().any(|(nm, b, ns)| *b == q && *ns != HOIST_NS_TYPE && nm.as_slice() == tok(nk)) => {
                            let mut e = 0i32;
                            while q < toks.len() {
                                if toks[q].0 == 1 && tok(q) == b"{" {
                                    e += 1;
                                } else if toks[q].0 == 1 && tok(q) == b"}" {
                                    e -= 1;
                                    if e == 0 {
                                        break;
                                    }
                                }
                                q += 1;
                            }
                        }
                        b"{" => d += 1,
                        b"}" => {
                            if d == 0 {
                                break;
                            }
                            d -= 1;
                        }
                        _ => {}
                    }
                } else if toks[q].0 == 0 && tok(q) == tok(nk) {
                    local_toks[q] = true;
                }
                q += 1;
            }
        }
        let mut at: Vec<(usize, usize)> = vec![]; // (トークン, renames の番号)
        // (not an attribute's tokens or a macro's name: `#[derive(Debug)]` / `write!(..)` beside a nested `struct Debug` /
        // `fn write`)
        let no_item = macro_ns_toks(&toks, s);
        for (ri, r) in renames.iter().enumerate() {
            let mut k = r.0 + 1;
            while k < r.1 {
                if toks[k].0 == 0 && !no_item[k] && &s[toks[k].1..toks[k].2] == r.2.as_slice() && !number_suffix(s, toks[k].1) {
                    at.push((k, ri));
                }
                k += 1;
            }
        }
        // (同じトークンに複数あれば、その位置を囲むいちばん内側の block のもの: Rust の名前の見え方と同じ。
        //  `fn m` in a fn's body beside another `fn m` in a block of it: the block's tokens take the block's name, or the
        //  two would be renamed alike on each redo)
        // (in the namespace the token is in, where its neighbours tell: `m(` / `fn m` a value, `m::` / `struct m` /
        // `-> m` a type. `fn m` beside `{ struct m {} m() }`: the call takes the fn's name, not the struct's)
        let tok_ns = |k: usize| -> u8 {
            let prev = if k >= 1 { tok(k - 1) } else { b"" };
            let next = if k + 1 < toks.len() { tok(k + 1) } else { b"" };
            // (a turbofish, `m::<u8>`: a call after it, `m::<u8>()`, is a fn's; a path on, `m::<u8>::mk()`, a type's)
            let turbofish = k + 3 < toks.len() && next == b":" && tok(k + 2) == b":" && tok(k + 3) == b"<";
            let after_turbofish = || -> &[u8] {
                let mut d = 0i32;
                for q in k + 3..toks.len() {
                    if toks[q].0 != 1 {
                        continue;
                    }
                    match tok(q) {
                        b"<" => d += 1,
                        b">" if !(toks[q - 1].0 == 1 && tok(q - 1) == b"-") => {
                            d -= 1;
                            if d == 0 {
                                return if q + 1 < toks.len() { tok(q + 1) } else { b"" };
                            }
                        }
                        b";" | b"{" | b"}" => break,
                        _ => {}
                    }
                }
                b""
            };
            if matches!(prev, b"fn" | b"static" | b"const") {
                HOIST_NS_VALUE
            } else if turbofish {
                match after_turbofish() {
                    b"(" => HOIST_NS_VALUE,
                    b":" => HOIST_NS_TYPE,
                    _ => HOIST_NS_BOTH,
                }
            } else if matches!(prev, b"struct" | b"enum" | b"union" | b"trait" | b"type" | b"mod" | b"impl" | b"dyn")
                // (`m<T>`, generic arguments: a fn's name is not compared)
                || next == b"<"
                // (`-> m` and `m::`: a symbol is a token of one byte)
                || (k >= 2 && prev == b">" && tok(k - 2) == b"-")
                || (k + 2 < toks.len() && next == b":" && tok(k + 2) == b":")
            {
                HOIST_NS_TYPE
            } else if prev == b":"
                && !(k >= 2 && tok(k - 2) == b":")
                && next != b":"
                && (matches!(next, b"=" | b";" | b")") || (k >= 3 && matches!(tok(k - 3), b"let" | b"mut" | b"(" | b"|")))
            {
                // (an annotation, `let a: m = ..` / `let mut a: m;` / `fn h(x: m)`: not a struct literal's field,
                // `S { f: m, .. }`, which none of these tells)
                HOIST_NS_TYPE
            } else if next == b"(" {
                HOIST_NS_VALUE
            } else if next == b"{"
                && (matches!(prev, b"(" | b"," | b"{" | b";" | b"}" | b"return")
                    || prev == b"=" && !(k >= 2 && toks[k - 2].0 == 1 && b"=!<>+-*/%&|^".contains(&s[toks[k - 2].1]))
                    || prev == b">" && k >= 2 && tok(k - 2) == b"=")
            {
                // (a struct literal, `let a = m { .. }` / `{ m { .. } }` / `=> m { .. }`: not `x == m {` or
                // `if m {`, where `{` opens a block)
                HOIST_NS_TYPE
            } else {
                HOIST_NS_BOTH
            }
        };
        let fits = |k: usize, ri: usize| {
            let (a, b) = (tok_ns(k), rename_ns[ri]);
            a == HOIST_NS_BOTH || b == HOIST_NS_BOTH || a == b
        };
        // (a local's tokens keep the name, but for a type's use, `let _: n = n {};`: a `let` shadows no type. Not
        // `n <`, which is a comparison of the local, `if n < 3`, unless an annotation's, `x: n<u8>`)
        at.retain(|&(k, _)| {
            !local_toks[k] || (tok_ns(k) == HOIST_NS_TYPE && !(k + 1 < toks.len() && tok(k + 1) == b"<" && !(k >= 1 && tok(k - 1) == b":")))
        });
        // (a candidate of the other namespace stays only where none fits)
        let mut has_fit = vec![false; toks.len()];
        for &(k, ri) in &at {
            if fits(k, ri) {
                has_fit[k] = true;
            }
        }
        at.retain(|&(k, ri)| fits(k, ri) || !has_fit[k]);
        at.sort_by(|x, y| x.0.cmp(&y.0).then(renames[y.1].0.cmp(&renames[x.1].0)));
        at.dedup_by_key(|x| x.0);
        // (a foreign function's declaration, `fn abs(x: i32) -> i32;` with no body, renamed: it keeps its symbol, a
        // `#[link_name = "abs"]` before it unless it has one)
        let foreign_decl = |k: usize| -> Option<usize> {
            if k < 1 || !(toks[k - 1].0 == 0 && tok(k - 1) == b"fn") {
                return None;
            }
            let mut d = 0i32;
            let mut j = k + 1;
            while j < toks.len() {
                if toks[j].0 == 1 {
                    match tok(j) {
                        b"(" | b"[" => d += 1,
                        b")" | b"]" => d -= 1,
                        b";" if d == 0 => break,
                        b"{" if d == 0 => return None,
                        _ => {}
                    }
                }
                j += 1;
            }
            let mut st = k - 1;
            loop {
                if st >= 1 && toks[st - 1].0 == 0 && matches!(tok(st - 1), b"unsafe" | b"safe" | b"pub") {
                    st -= 1;
                } else if st >= 4 && is_sym(st - 1, b")") && toks[st - 2].0 == 0 && is_sym(st - 3, b"(") && tok(st - 4) == b"pub" {
                    // (`pub(crate)` / `pub(super)`: the attribute goes before it)
                    st -= 4;
                } else {
                    break;
                }
            }
            // (its attributes, back to the block's `{` or the previous item's `;`)
            let mut a = st;
            while a >= 1 && !(toks[a - 1].0 == 1 && matches!(tok(a - 1), b"{" | b";" | b"}")) {
                a -= 1;
                if toks[a].0 == 0 && tok(a) == b"link_name" {
                    return None;
                }
            }
            Some(st)
        };
        let mut out = Vec::with_capacity(s.len() + 64);
        let mut last = 0;
        for &(k, ri) in &at {
            let t = toks[k];
            if let Some(st) = foreign_decl(k) {
                out.extend_from_slice(&s[last..toks[st].1]);
                out.extend_from_slice(format!("#[link_name = \"{}\"] ", String::from_utf8_lossy(&renames[ri].2)).as_bytes());
                last = toks[st].1;
            }
            out.extend_from_slice(&s[last..t.1]);
            out.extend_from_slice(&renames[ri].3);
            last = t.2;
        }
        out.extend_from_slice(&s[last..]);
        let renamed = String::from_utf8(out).ok()?;
        // (名前を変えるトークンが無ければ (変えても同じなら) やり直さない: 同じ入力で回り続ける)
        if renamed.as_str() == src || redo >= 8 {
            return hoist_plain(s, &moves);
        }
        return match hoist_parts_at(&renamed, redo + 1) {
            Some(r) => Some(r),
            None => Some((renamed, vec![])),
        };
    }
    hoist_plain(s, &moves)
}

/// 移すものを空白にした本文と、移したもの
fn hoist_plain(s: &[u8], moves: &[(usize, usize)]) -> Option<(String, Vec<(usize, String)>)> {
    let mut out = s.to_vec();
    let mut parts: Vec<(usize, String)> = vec![];
    for &(a, b) in moves {
        parts.push((a, String::from_utf8(s[a..b].to_vec()).ok()?));
        for x in &mut out[a..b] {
            if *x != b'\n' {
                *x = b' ';
            }
        }
    }
    Some((String::from_utf8(out).ok()?, parts))
}

/// `thread_local! { static N: T = init; }` を、スレッドごとの値 (`__exr_tls_slot`、遅延初期化) を指す鍵と `N.with(|v| ..)` を持つ unit struct にする
/// (zustc の生成するプログラムはスレッドを作らないので、スレッドごとの値は 1 つでよい)。
/// 展開は 1 行に収め、後ろの行番号は変えない
pub fn expand_thread_local(src: &str) -> Option<String> {
    if !src.contains("thread_local") {
        return None;
    }
    let s = src.as_bytes();
    let toks = tokenize(s);
    let is_sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && s[toks[k].1] == c && toks[k].2 == toks[k].1 + 1;
    // (the tokens a..b; none when b <= a: `thread_local! { static X: ]` has no type before its end)
    let text = |a: usize, b: usize| -> String { if b <= a { String::new() } else { String::from_utf8_lossy(&s[toks[a].1..toks[b - 1].2]).into_owned() } };
    let mut out = String::new();
    let mut last = 0;
    let mut k = 0;
    while k < toks.len() {
        let is_tl = toks[k].0 == 0 && &s[toks[k].1..toks[k].2] == b"thread_local".as_slice() && is_sym(k + 1, b'!');
        if !is_tl || !(is_sym(k + 2, b'{') || is_sym(k + 2, b'(')) {
            k += 1;
            continue;
        }
        // 閉じ括弧
        let mut d = 0i32;
        let mut e = k + 2;
        while e < toks.len() {
            if is_sym(e, b'{') || is_sym(e, b'(') || is_sym(e, b'[') {
                d += 1;
            } else if is_sym(e, b'}') || is_sym(e, b')') || is_sym(e, b']') {
                d -= 1;
                if d == 0 {
                    break;
                }
            }
            e += 1;
        }
        if e >= toks.len() {
            return None;
        }
        let mut end = toks[e].2;
        if is_sym(e + 1, b';') {
            end = toks[e + 1].2;
        }
        let mut expanded = String::new();
        let mut j = k + 3;
        while j < e {
            // [属性] [pub(..)] static NAME: TYPE = [const] INIT;
            while is_sym(j, b'#') {
                let mut d = 0i32;
                j += 1;
                while j < e {
                    if is_sym(j, b'[') {
                        d += 1;
                    } else if is_sym(j, b']') {
                        d -= 1;
                        if d == 0 {
                            j += 1;
                            break;
                        }
                    }
                    j += 1;
                }
            }
            let mut vis = String::new();
            let v0 = j;
            while j < e && !(toks[j].0 == 0 && &s[toks[j].1..toks[j].2] == b"static".as_slice()) {
                j += 1;
            }
            if j > v0 {
                vis = text(v0, j);
            }
            if j + 2 >= e || !is_sym(j + 2, b':') {
                return None;
            }
            let name = text(j + 1, j + 2);
            let t0 = j + 3;
            let mut t1 = t0;
            while t1 < e && !is_sym(t1, b'=') {
                t1 += 1;
            }
            let ty = text(t0, t1);
            let mut i0 = t1 + 1;
            if i0 < e && toks[i0].0 == 0 && &s[toks[i0].1..toks[i0].2] == b"const".as_slice() {
                i0 += 1;
            }
            let mut i1 = i0;
            let mut d = 0i32;
            while i1 < e {
                if is_sym(i1, b'{') || is_sym(i1, b'(') || is_sym(i1, b'[') {
                    d += 1;
                } else if is_sym(i1, b'}') || is_sym(i1, b')') || is_sym(i1, b']') {
                    d -= 1;
                } else if d == 0 && is_sym(i1, b';') {
                    break;
                }
                i1 += 1;
            }
            if i1 == i0 {
                return None;
            }
            let init = text(i0, i1);
            // (std's LocalKey<Cell<T>> / LocalKey<RefCell<T>> helpers, on the slot once `with` has filled it)
            let tt = ty.trim();
            let slot = format!("{{ self.with(|_| ()); unsafe {{ (*__exr_tls_slot::<{ty}>(&{name}__tk)).as_ref().unwrap() }} }}");
            // (null: the thread's value is being dropped, or was (std's AccessError). An initializer that made the value
            // itself first (it called `with` again) is outlived by its own: that one replaces it, and the other is
            // dropped after, seeing the new value, as std's)
            let extra = if let Some(inner) = tt.strip_prefix("Cell<").and_then(|r| r.strip_suffix('>')) {
                format!(
                    "{vis} fn set(&'static self, v: {inner}) {{ {slot}.set(v) }} \
                     {vis} fn get(&'static self) -> {inner} {{ {slot}.get() }} \
                     {vis} fn take(&'static self) -> {inner} {{ {slot}.take() }} \
                     {vis} fn replace(&'static self, v: {inner}) -> {inner} {{ {slot}.replace(v) }} "
                )
            } else if let Some(inner) = tt.strip_prefix("RefCell<").and_then(|r| r.strip_suffix('>')) {
                format!(
                    "{vis} fn set(&'static self, v: {inner}) {{ *{slot}.borrow_mut() = v; }} \
                     {vis} fn take(&'static self) -> {inner} {{ {slot}.take() }} \
                     {vis} fn replace(&'static self, v: {inner}) -> {inner} {{ {slot}.replace(v) }} \
                     {vis} fn with_borrow<R, F: FnOnce(&{inner}) -> R>(&'static self, f: F) -> R {{ f(&*{slot}.borrow()) }} \
                     {vis} fn with_borrow_mut<R, F: FnOnce(&mut {inner}) -> R>(&'static self, f: F) -> R {{ f(&mut *{slot}.borrow_mut()) }} "
                )
            } else {
                String::new()
            };
            expanded.push_str(&format!(
                "#[allow(non_camel_case_types)] {vis} struct {name}__Key; static {name}__tk: __TlsKey = __TlsKey::new(); \
                 {vis} static {name}: {name}__Key = {name}__Key; impl {name}__Key {{ {vis} fn with<R, F: FnOnce(&{ty}) -> R>(&'static self, f: F) -> R \
                 {{ let s = __exr_tls_slot::<{ty}>(&{name}__tk); if s.is_null() {{ __exr_tls_destroyed() }} unsafe {{ if (*s).is_none() {{ let v = {init}; let old = core::mem::replace(&mut *s, Some(v)); __exr_tls_made::<{ty}>(&{name}__tk); core::mem::drop(old); }} f((*s).as_ref().unwrap()) }} }} \
                 {vis} fn try_with<R, F: FnOnce(&{ty}) -> R>(&'static self, f: F) -> Result<R, __TlsAccessError> {{ if __exr_tls_slot::<{ty}>(&{name}__tk).is_null() {{ return Err(__exr_access_error()); }} Ok(self.with(f)) }} {extra}}} "
            ));
            j = i1 + 1;
        }
        // (`std::thread_local!` / `::std::thread_local!`: the path goes too)
        let mut ak = k;
        while ak >= 3 && is_sym(ak - 1, b':') && is_sym(ak - 2, b':') && toks[ak - 3].0 == 0 {
            ak -= 3;
        }
        if ak >= 2 && is_sym(ak - 1, b':') && is_sym(ak - 2, b':') {
            ak -= 2;
        }
        let a = toks[ak].1.max(last);
        out.push_str(&src[last..a]);
        out.push_str(expanded.replace('\n', " ").as_str());
        for _ in 0..s[a..end].iter().filter(|&&c| c == b'\n').count() {
            out.push('\n');
        }
        last = end;
        k = if is_sym(e + 1, b';') { e + 2 } else { e + 1 };
    }
    if last == 0 {
        return None;
    }
    out.push_str(&src[last..]);
    Some(out)
}

/// Which traits are in scope where, when two scopes (modules, or a module and the root) define traits of one name
/// (`mod a { trait M }`, `mod b { trait M }`): flattening renames them (`a__M`, `b__M`), but a method call `x.m()`
/// names no trait, and the call has to use the impl of the trait in scope at it. For each scope, its lines (1-based,
/// inclusive; the root is 1..=u32::MAX, the modules inside it narrower) and the flattened names of the traits of a
/// colliding name that are in scope there: defined in it, or brought in by `use` (also `as`, and globs). Empty when no
/// trait name collides (then nothing needs it).
pub fn trait_scopes(src: &str) -> Vec<(u32, u32, Vec<String>)> {
    trait_scopes_with(src, src.contains("mod"))
}

/// trait_scopes, with whether src contains `mod` already known (one search of the text for both uses)
pub fn trait_scopes_with(src: &str, has_mod: bool) -> Vec<(u32, u32, Vec<String>)> {
    trait_scopes_in(src, has_mod, None)
}

/// trait_scopes_with, with src's tokens (`tokenize(src)`) when the caller has them already
pub fn trait_scopes_in(src: &str, has_mod: bool, src_toks: Option<&[(u8, usize, usize)]>) -> Vec<(u32, u32, Vec<String>)> {
    if !has_mod || !src.contains("trait") {
        return vec![];
    }
    // (a quick look first: the names after the word `trait` and any whitespace, newlines too, by text; no name twice,
    // nothing to do. Most programs stop here, without tokenizing. A comment after `trait` leaves it to the tokens)
    {
        let b = src.as_bytes();
        let mut names: Vec<&str> = vec![];
        let mut unsure = false;
        for (i, _) in src.match_indices("trait") {
            let before_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
            let after = &src[i + 5..];
            if !before_ok || !after.starts_with(|c: char| c.is_whitespace() || c == '/') {
                continue;
            }
            let r = after.trim_start();
            if r.starts_with('/') {
                unsure = true;
                break;
            }
            let n = &r[..r.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(r.len())];
            if !n.is_empty() {
                names.push(n);
            }
        }
        names.sort_unstable();
        if !unsure && !names.windows(2).any(|w| w[0] == w[1]) {
            return vec![];
        }
    }
    let s = src.as_bytes();
    let own;
    let toks: &[(u8, usize, usize)] = match src_toks {
        Some(t) => t,
        None => {
            own = tokenize(s);
            &own
        }
    };
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && s[toks[k].1] == c && toks[k].2 == toks[k].1 + 1;
    let is_id = |k: usize, w: &[u8]| toks[k].0 == 0 && tok(k) == w;
    // (modules: path, opening `{`, closing `}`; as rename_module_collisions_fix finds them)
    let mut mods: Vec<(Vec<Vec<u8>>, usize, usize)> = vec![];
    let mut stack: Vec<(usize, i32)> = vec![];
    // (traits: name, scope; usize::MAX is the root)
    let mut traits: Vec<(Vec<u8>, usize)> = vec![];
    let mut depth = 0i32;
    let mut k = 0;
    while k < toks.len() {
        if is_id(k, b"mod") && k + 2 < toks.len() && toks[k + 1].0 == 0 && is_sym(k + 2, b'{') {
            let mut path = stack.last().map(|&(m, _)| mods[m].0.clone()).unwrap_or_default();
            path.push(tok(k + 1).to_vec());
            mods.push((path, k + 2, usize::MAX));
            stack.push((mods.len() - 1, depth));
            depth += 1;
            k += 3;
            continue;
        }
        if is_sym(k, b'{') {
            depth += 1;
        } else if is_sym(k, b'}') {
            depth -= 1;
            if stack.last().is_some_and(|&(_, d)| d == depth) {
                let (m, _) = stack.pop().unwrap();
                mods[m].2 = k;
            }
        } else if is_id(k, b"trait") && k + 1 < toks.len() && toks[k + 1].0 == 0 && stack.last().is_none_or(|&(_, d)| d + 1 == depth) {
            traits.push((tok(k + 1).to_vec(), stack.last().map_or(usize::MAX, |&(m, _)| m)));
        }
        k += 1;
    }
    let colliding: Vec<&Vec<u8>> = traits
        .iter()
        .filter(|(n, sc)| traits.iter().any(|(n2, sc2)| n2 == n && sc2 != sc))
        .map(|(n, _)| n)
        .collect();
    if colliding.is_empty() {
        return vec![];
    }
    // (the flattened name: `a__b__M`, or `M` at the root, which keeps its name)
    let flat = |m: usize, n: &[u8]| -> String {
        let mut v = String::new();
        if m != usize::MAX {
            for p in &mods[m].0 {
                v.push_str(&String::from_utf8_lossy(p));
                v.push_str("__");
            }
        }
        v.push_str(&String::from_utf8_lossy(n));
        v
    };
    let mod_of_path = |p: &[Vec<u8>]| -> Option<usize> {
        if p.is_empty() {
            return Some(usize::MAX);
        }
        mods.iter().position(|m| m.0 == p)
    };
    let scope_of = |k: usize| -> usize {
        // (the innermost module around token k: the last one opened before it that closes after it)
        let mut best = usize::MAX;
        for (i, m) in mods.iter().enumerate() {
            if m.1 < k && k < m.2 {
                best = i;
            }
        }
        best
    };
    let resolve = |from: usize, segs: &[Vec<u8>]| -> Option<usize> {
        let mut base: Vec<Vec<u8>> = if from == usize::MAX { vec![] } else { mods[from].0.clone() };
        let mut rest = segs;
        if rest.first().is_some_and(|x| x.as_slice() == b"crate") {
            base.clear();
            rest = &rest[1..];
        } else if rest.first().is_some_and(|x| x.as_slice() == b"self") {
            rest = &rest[1..];
        } else if rest.first().is_some_and(|x| x.as_slice() == b"super") {
            while rest.first().is_some_and(|x| x.as_slice() == b"super") {
                base.pop();
                rest = &rest[1..];
            }
        } else {
            // (a relative path; failing that, from the crate root (edition 2015 style))
            let mut rel = base.clone();
            rel.extend(rest.iter().cloned());
            if let Some(m) = mod_of_path(&rel) {
                return Some(m);
            }
            base.clear();
        }
        base.extend(rest.iter().cloned());
        mod_of_path(&base)
    };
    let n_scopes = mods.len() + 1;
    let ix = |sc: usize| if sc == usize::MAX { mods.len() } else { sc };
    let mut visible: Vec<Vec<String>> = vec![vec![]; n_scopes];
    for (n, sc) in &traits {
        if colliding.contains(&n) {
            visible[ix(*sc)].push(flat(*sc, n));
        }
    }
    // (the `use` items: (scope, module path, name or `*`, the name used). Resolved below to a fixed point, so a trait
    // re-exported by `pub use` in one module and imported from there in another is followed)
    let mut uses: Vec<(usize, Vec<Vec<u8>>, Vec<u8>, Vec<u8>)> = vec![];
    let mut k = 0;
    while k < toks.len() {
        if !is_id(k, b"use") {
            k += 1;
            continue;
        }
        let sc = scope_of(k);
        let mut e = k;
        while e < toks.len() && !is_sym(e, b';') {
            e += 1;
        }
        let mut items = vec![];
        use_walk(&toks, s, k + 1, e, vec![], &mut items);
        for (path, local) in items {
            if local.as_slice() == b"*" {
                uses.push((sc, path, b"*".to_vec(), local));
            } else if let Some((name, mpath)) = path.split_last() {
                uses.push((sc, mpath.to_vec(), name.clone(), local));
            }
        }
        k = e + 1;
    }
    // (scope, name used there, the flattened trait, by a glob): what each scope imported, including what it
    // re-exports
    let mut imported: Vec<(usize, Vec<u8>, String, bool)> = vec![];
    // (a name a scope defines or imports by name shadows the same name from a glob: `use a::*; use b::M;` is b's M)
    let named_in = |imported: &[(usize, Vec<u8>, String, bool)], sc: usize, n: &[u8]| {
        traits.iter().any(|(t, tsc)| *tsc == sc && t.as_slice() == n) || imported.iter().any(|(isc, i, _, g)| *isc == sc && i.as_slice() == n && !g)
    };
    // (each pass computes every binding again from what the previous pass found, so one forwarded before a named
    // import shadowed it is dropped when it is: `pub use a::*; mod g { use super::M; } pub use b::M;` gives g only
    // b's M. The bindings grow pass by pass to the fixed point, as rustc's import resolution; a cycle that does not
    // settle stops after enough passes for any chain)
    let max_passes = (uses.len() + 1) * (traits.len() + 1) + 1;
    for _ in 0..max_passes {
        // (what m passes on: its bindings, less a glob's that a named one shadows)
        let effective = |m: usize| -> Vec<(Vec<u8>, String)> {
            imported.iter().filter(|(isc, n, _, g)| *isc == m && !(*g && named_in(&imported, m, n))).map(|(_, n, f, _)| (n.clone(), f.clone())).collect()
        };
        let mut next: Vec<(usize, Vec<u8>, String, bool)> = vec![];
        for (sc, mpath, name, local) in &uses {
            let Some(m) = resolve(*sc, mpath) else { continue };
            let glob = name.as_slice() == b"*";
            let found: Vec<(Vec<u8>, String)> = if glob {
                // (a glob: the module's traits of a colliding name, and what it imported)
                let mut v: Vec<(Vec<u8>, String)> =
                    traits.iter().filter(|(n, tsc)| *tsc == m && colliding.contains(&n)).map(|(n, _)| (n.clone(), flat(m, n))).collect();
                v.extend(effective(m));
                v
            } else if traits.iter().any(|(n, tsc)| n == name && *tsc == m) {
                if !colliding.contains(&name) {
                    continue;
                }
                vec![(local.clone(), flat(m, name))]
            } else {
                // (what m binds to the name)
                effective(m).into_iter().filter(|(n, _)| n == name).map(|(_, f)| (local.clone(), f)).collect()
            };
            for (n, f) in found {
                match next.iter_mut().find(|(isc, n2, f2, _)| isc == sc && *n2 == n && *f2 == f) {
                    // (one that came by a glob and by name as well: it is a named binding)
                    Some(e) => e.3 &= glob,
                    None => next.push((*sc, n, f, glob)),
                }
            }
        }
        if next == imported {
            break;
        }
        imported = next;
    }
    let named = |sc: usize, n: &[u8]| named_in(&imported, sc, n);
    for (sc, n, f, glob) in &imported {
        if *glob && named(*sc, n) {
            continue;
        }
        if !visible[ix(*sc)].contains(f) {
            visible[ix(*sc)].push(f.clone());
        }
    }
    // (the line of token k: newline offsets found once, then a binary search)
    let nls: Vec<usize> = s.iter().enumerate().filter(|(_, c)| **c == b'\n').map(|(i, _)| i).collect();
    let line_of = |k: usize| -> u32 { (nls.partition_point(|&p| p < toks[k.min(toks.len() - 1)].1) + 1) as u32 };
    let mut out = vec![(1, u32::MAX, std::mem::take(&mut visible[mods.len()]))];
    for (i, m) in mods.iter().enumerate() {
        out.push((line_of(m.1), line_of(m.2), std::mem::take(&mut visible[i])));
    }
    out
}

/// The names a `use` tree brings in: (the path, the name used). `*` for a glob. (a tree walk: the current prefix,
/// the position)
fn use_walk(
    toks: &[(u8, usize, usize)],
    s: &[u8],
    j: usize,
    end: usize,
    prefix: Vec<Vec<u8>>,
    out: &mut Vec<(Vec<Vec<u8>>, Vec<u8>)>,
) -> usize {
    use_walk_in(toks, s, j, end, prefix, out, &mut vec![])
}

/// use_walk, and the places in out of the `self` entries (`use a::m::{self};`), which bring the type namespace only:
/// the module or the type m, not a fn m beside it
fn use_walk_in(
    toks: &[(u8, usize, usize)],
    s: &[u8],
    mut j: usize,
    end: usize,
    prefix: Vec<Vec<u8>>,
    out: &mut Vec<(Vec<Vec<u8>>, Vec<u8>)>,
    selfs: &mut Vec<usize>,
) -> usize {
    let tk = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let mut path = prefix.clone();
    while j < end {
        let t = tk(j);
        if toks[j].0 == 0 {
            // (`self` first in a tree is the prefix itself: `use super::Sign::{self, Minus};` brings `Sign` (num-bigint),
            // `use crate::util::{alphabet::{self, ByteClasses}};` the module `alphabet`, as `self as x` brings it as x)
            if t == b"self" && path.len() == prefix.len() && !prefix.is_empty() && !(j + 1 < end && tk(j + 1) == b":") {
                let as_x = j + 2 < end && toks[j + 1].0 == 0 && tk(j + 1) == b"as";
                // (`use super::{self};` names no item: left as it was)
                let kw_prefix = prefix.last().is_some_and(|p| matches!(p.as_slice(), b"self" | b"super" | b"crate"));
                if as_x || !kw_prefix {
                    let local = if as_x { tk(j + 2).to_vec() } else { prefix.last().cloned().unwrap_or_default() };
                    selfs.push(out.len());
                    out.push((prefix.clone(), local));
                    path = prefix.clone();
                    j += if as_x { 3 } else { 1 };
                    continue;
                }
            }
            if t == b"as" {
                // 別名
                if j + 1 < end {
                    let alias = tk(j + 1).to_vec();
                    out.push((path.clone(), alias));
                    path = prefix.clone();
                    j += 2;
                    continue;
                }
            }
            path.push(t.to_vec());
            j += 1;
            let next_is_sep = j + 1 < end && tk(j) == b":" && tk(j + 1) == b":";
            let next_is_as = j < end && toks[j].0 == 0 && tk(j) == b"as";
            if !next_is_sep && !next_is_as {
                let last = path.last().cloned().unwrap_or_default();
                out.push((path.clone(), last));
                path = prefix.clone();
            }
        } else if t == b":" {
            j += 1;
        } else if t == b"{" {
            j = use_walk_in(toks, s, j + 1, end, path.clone(), out, selfs);
            path = prefix.clone();
        } else if t == b"}" {
            return j + 1;
        } else {
            // `,` / `*` (`use m::*`: 名前の代わりに `*` を記録する)
            if t == b"*" {
                out.push((path.clone(), b"*".to_vec()));
            }
            if t != b"," {
                path = prefix.clone();
            }
            j += 1;
            if t == b"," {
                path = prefix.clone();
            }
        }
    }
    j
}

/// モジュールの中の名前を平らにしてもぶつからないようにする: 2 つ以上のスコープ (モジュール・ルート) で定義される名前は、
/// モジュールの中の定義を `m__Name` (入れ子なら `a__b__Name`) に変え、参照も直す:
/// - そのモジュールの中の裸の名前、`use crate::m::{Name, X as Y}` で取り込んだ名前 (別名も)
/// - `m::Name` / `crate::m::Name` / `self::Name` / `super::Name` のパス
/// (フィールド `x.name` / `name: T` / メソッド `.name(..)` は変えない)
pub fn rename_module_collisions(src: &str) -> Option<String> {
    rename_module_collisions_in(src, &[])
}

/// crates: クレートとして読み込んだモジュール (`pub mod geom { .. }`) の名前。その中のアイテムはぶつからなくても
/// 必ず `geom__Name` にし (どのクレートと一緒にコンパイルしても同じシンボルになるように)、中の `crate::` はそのモジュールを指す
/// (an entry `owner:alias=crate` is a dependency the crate `owner` names `alias`, a renamed one; `:alias=crate` for
/// the root of the source: session::build's CrateSpec::aliases)
pub fn rename_module_collisions_in(src: &str, crates: &[String]) -> Option<String> {
    rename_module_collisions_fix(src, crates, &mut vec![])
}

/// fix: 名前を変えて桁がずれた所 (行, 新しいテキストでの桁, その行でそこから後ろのずれの合計)。
/// 元の桁 = 桁 - ずれ (その行で桁が col 以上の最後の項)
/// Whether a `use` renames an item of std (`use std::cell::LazyCell as Lazy;`): the name it brings in is that path
fn renames_std(src: &str) -> bool {
    // (the source can be large and most have none: `contains` first (it is much faster than `match_indices` over a
    // large text), then the lines that have a `use `)
    if !src.contains("use ") {
        return false;
    }
    let b = src.as_bytes();
    let mut off = 0;
    for line in src.split('\n') {
        let start = off;
        off += line.len() + 1;
        if !line.contains("use ") {
            continue;
        }
        for (j, _) in line.match_indices("use ") {
            let i = start + j;
            if i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
                continue;
            }
            let r = src[i + 4..].trim_start();
            let r = r.strip_prefix("::").unwrap_or(r);
            if !(r.starts_with("std::") || r.starts_with("core::") || r.starts_with("alloc::")) {
                continue;
            }
            let stmt = &r[..r.find(';').unwrap_or(r.len())];
            if stmt.match_indices(" as ").any(|(j, _)| stmt[j + 4..].trim_start().starts_with(|c: char| c.is_alphabetic())) {
                return true;
            }
        }
    }
    false
}

pub fn rename_module_collisions_fix(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>) -> Option<String> {
    let has_mod = src.contains("mod");
    // (only without modules: with them the pass runs anyway)
    let std_renames = !has_mod && renames_std(src);
    if !has_mod && !std_renames {
        return None;
    }
    let s = src.as_bytes();
    let toks = tokenize(s);
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && s[toks[k].1] == c && toks[k].2 == toks[k].1 + 1;
    let is_id = |k: usize, w: &[u8]| toks[k].0 == 0 && tok(k) == w;
    // モジュール: (名前の列 (パス), 開き `{`, 閉じ `}`)
    let mut mods: Vec<(Vec<Vec<u8>>, usize, usize)> = vec![];
    let mut stack: Vec<(usize, i32)> = vec![]; // (mods の番号, 開いたときの深さ)
    let mut depth = 0i32;
    let mut k = 0;
    while k < toks.len() {
        if is_id(k, b"mod") && k + 2 < toks.len() && toks[k + 1].0 == 0 && is_sym(k + 2, b'{') {
            let mut path = stack.last().map(|&(m, _)| mods[m].0.clone()).unwrap_or_default();
            path.push(tok(k + 1).to_vec());
            mods.push((path, k + 2, usize::MAX));
            stack.push((mods.len() - 1, depth));
            depth += 1;
            k += 3;
            continue;
        }
        if is_sym(k, b'{') {
            depth += 1;
        } else if is_sym(k, b'}') {
            depth -= 1;
            if stack.last().is_some_and(|&(_, d)| d == depth) {
                let (m, _) = stack.pop().unwrap();
                mods[m].2 = k;
            }
        }
        k += 1;
    }
    // (no module: only for a `use` that renames an item of std)
    if mods.is_empty() && !std_renames && !renames_std(src) {
        return None;
    }
    // トークン k のスコープ (いちばん内側のモジュール。なければ usize::MAX = ルート)。
    // (mods は開く順なので、後のもの (内側) で上書きすれば いちばん内側になる)
    let mut scope_tab: Vec<usize> = vec![usize::MAX; toks.len()];
    for (i, m) in mods.iter().enumerate() {
        let end = m.2.min(toks.len());
        let mut k = m.1 + 1;
        while k < end {
            scope_tab[k] = i;
            k += 1;
        }
    }
    let scope_of = |k: usize| -> usize { scope_tab[k] };
    // (`{` のトークン → それを開くモジュール、パス → モジュール)
    let mut open_of: LMap<usize, usize> = LMap::default();
    let mut mod_of_path: LMap<Vec<Vec<u8>>, usize> = LMap::default();
    for (i, m) in mods.iter().enumerate() {
        open_of.insert(m.1, i);
        if !mod_of_path.contains_key(&m.0) {
            mod_of_path.insert(m.0.clone(), i);
        }
    }
    // 各スコープのトップレベルのアイテムの名前 (そのスコープの深さの `fn x` / `struct X` など)
    let mut defs: Vec<(Vec<u8>, usize, usize)> = vec![]; // (名前, スコープ, 名前のトークン)
    let mut extern_close: Vec<i32> = vec![];
    let mut extern_defs: Vec<(usize, Vec<u8>, usize)> = vec![]; // (スコープ, 名前, 宣言の先頭のトークン)
    let mut depth = 0i32;
    let mut scope_depth: Vec<(usize, i32)> = vec![(usize::MAX, 0)];
    let mut k = 0;
    while k < toks.len() {
        if let Some(&m) = open_of.get(&k) {
            depth += 1;
            scope_depth.push((m, depth));
            k += 1;
            continue;
        }
        if is_sym(k, b'{') {
            depth += 1;
            // (`extern "C" { fn f(); }` at a module's level: its fns are the module's items)
            let at_scope = scope_depth.last().is_some_and(|&(_, d)| d == depth - 1);
            let ext = k >= 1 && (is_id(k - 1, b"extern") || (k >= 2 && toks[k - 1].0 == 2 && is_id(k - 2, b"extern")));
            if at_scope && ext {
                let m = scope_depth.last().unwrap().0;
                scope_depth.push((m, depth));
                extern_close.push(depth);
            }
        } else if is_sym(k, b'}') {
            if extern_close.last() == Some(&depth) {
                extern_close.pop();
                scope_depth.pop();
            } else if scope_depth.last().is_some_and(|&(m, _)| m != usize::MAX && mods[m].2 == k) {
                scope_depth.pop();
            }
            depth -= 1;
        } else if toks[k].0 == 0 && scope_depth.last().is_some_and(|&(_, d)| d == depth) {
            let kw = tok(k);
            // (`*const T` の const / `&'static` の static は定義でない)
            let after_star = k >= 1 && is_sym(k - 1, b'*');
            let after_tick = toks[k].1 >= 1 && s[toks[k].1 - 1] == b'\'';
            // (nor a const generic parameter, `fn f<const N: usize, const M: usize>`: an item is never after `<` / `,`)
            let generic = kw == b"const" && k >= 1 && (is_sym(k - 1, b'<') || is_sym(k - 1, b','));
            if matches!(kw, b"fn" | b"struct" | b"enum" | b"trait" | b"type" | b"const" | b"static" | b"union") && k + 1 < toks.len() && !after_star && !after_tick && !generic {
                let nk = if kw == b"static" && is_id(k + 1, b"mut") { k + 2 } else { k + 1 };
                // (`const fn f` は fn で数える。`const unsafe fn` / `const async fn` / `const extern "C" fn` も)
                let const_fn = kw == b"const" && (is_id(nk, b"fn") || is_id(nk, b"unsafe") || is_id(nk, b"async") || is_id(nk, b"extern"));
                // (a const generic parameter, `<W, const N: usize>`, follows `<` or `,`: it is no item of the module)
                let const_param = kw == b"const" && k >= 1 && (is_sym(k - 1, b'<') || is_sym(k - 1, b','));
                if nk < toks.len() && toks[nk].0 == 0 && !const_fn && !const_param && tok(nk) != b"_" {
                    let sc = scope_depth.last().unwrap().0;
                    defs.push((tok(nk).to_vec(), sc, nk));
                    // (a fn of an extern block: renamed, it keeps its symbol through a `#[link_name]` before the
                    // item (its `pub` or `fn`), unless it has one)
                    if kw == b"fn" && !extern_close.is_empty() {
                        // (the declaration's start: before `unsafe` / `safe` and the visibility, `pub(crate)` too)
                        let mut start = k;
                        while start >= 1 && (is_id(start - 1, b"unsafe") || is_id(start - 1, b"safe")) {
                            start -= 1;
                        }
                        if start >= 1 && is_sym(start - 1, b')') {
                            let mut j = start - 1;
                            while j > 0 && !is_sym(j, b'(') {
                                j -= 1;
                            }
                            if j >= 1 && is_id(j - 1, b"pub") {
                                start = j - 1;
                            }
                        } else if start >= 1 && is_id(start - 1, b"pub") {
                            start -= 1;
                        }
                        let mut j = start;
                        let mut has_ln = false;
                        while j >= 1 && is_sym(j - 1, b']') {
                            let mut a = j - 1;
                            while a > 0 && !is_sym(a, b'[') {
                                a -= 1;
                            }
                            has_ln |= (a..j).any(|x| is_id(x, b"link_name"));
                            j = a.saturating_sub(1);
                        }
                        if !has_ln {
                            extern_defs.push((sc, tok(nk).to_vec(), start));
                        }
                    }
                }
            }
        }
        k += 1;
    }
    // `use std::path::{Path, PathBuf};` などで std / core / alloc から取り込む型の名前: モジュールの中で同じ名前を
    // 定義していたら (zustc 自身の `parse::Path` など) ぶつかるものとして名前を変える
    let mut std_names: Vec<Vec<u8>> = vec![];
    {
        let mut j = 0;
        while j + 1 < toks.len() {
            if is_id(j, b"use") && (is_id(j + 1, b"std") || is_id(j + 1, b"core") || is_id(j + 1, b"alloc")) {
                let mut e = j + 1;
                while e < toks.len() && !is_sym(e, b';') {
                    if toks[e].0 == 0 && tok(e).first().is_some_and(|c| c.is_ascii_uppercase()) {
                        std_names.push(tok(e).to_vec());
                    }
                    e += 1;
                }
                j = e;
            }
            j += 1;
        }
    }
    let in_crate = |m: usize| -> bool { m != usize::MAX && mods[m].0.first().is_some_and(|r| crates.iter().any(|c| c.as_bytes() == r.as_slice())) };
    // (each name's scopes, read once (a scan of all the definitions per name is quadratic in a crate of many items):
    // the first scope defining it, whether another scope does too, whether one that is not the root does)
    let mut scopes_of: LMap<&[u8], (usize, bool, bool)> = LMap::default();
    for d in &defs {
        let e = scopes_of.entry(d.0.as_slice()).or_insert((d.1, false, false));
        e.1 |= e.0 != d.1;
        e.2 |= d.1 != usize::MAX;
    }
    // (a name two scopes define, or one a module defines that a `use` brings in from std too)
    let colliding = |n: &[u8]| -> bool {
        scopes_of.get(n).is_some_and(|&(_, multi, inner)| multi || inner && std_names.iter().any(|x| x.as_slice() == n))
    };
    let new_name = |m: usize, n: &[u8]| -> Vec<u8> {
        let mut v = Vec::new();
        for p in &mods[m].0 {
            v.extend_from_slice(p);
            v.extend_from_slice(b"__");
        }
        v.extend_from_slice(n);
        v
    };
    // 名前を変える定義: (スコープ, 元の名前) → 新しい名前
    let mut renamed: Vec<(usize, Vec<u8>, Vec<u8>)> = vec![];
    if std::env::var_os("ZUSTC_DEBUG_RENAME").is_some() {
        for d in &defs {
            if colliding(&d.0) {
                let line = s[..toks[d.2].1].iter().filter(|&&c| c == b'\n').count() + 1;
                eprintln!("collide {} scope {} line {line}", String::from_utf8_lossy(&d.0), d.1);
            }
        }
    }
    // (名前ごとの定義のスコープ: colliding を速くする)
    let mut def_set: LSet<(usize, Vec<u8>)> = LSet::default();
    for d in &defs {
        def_set.insert((d.1, d.0.clone()));
    }
    let mut renamed_map: LMap<(usize, Vec<u8>), Vec<u8>> = LMap::default();
    for d in &defs {
        if d.1 != usize::MAX && !renamed_map.contains_key(&(d.1, d.0.clone())) && (in_crate(d.1) || colliding(&d.0)) {
            let nn = new_name(d.1, &d.0);
            renamed_map.insert((d.1, d.0.clone()), nn.clone());
            renamed.push((d.1, d.0.clone(), nn));
        }
    }
    // (ぶつかる名前も `use .. as X` の別名もなければ何もしない)
    if renamed.is_empty() && !src.contains(" as ") {
        return None;
    }
    // (the definitions in the type namespace, a struct / enum / union / trait / type alias: (scope, name). A fn, const or
    // static is a value, which a path's first segment never names)
    let ty_defs: LSet<(usize, Vec<u8>)> =
        defs.iter().filter(|d| d.2 >= 1 && [&b"struct"[..], b"enum", b"union", b"trait", b"type"].iter().any(|w| is_id(d.2 - 1, w))).map(|d| (d.1, d.0.clone())).collect();
    // (a dependency under another name, `common = { package = "crypto-common" }` (rustc's `--extern common=..`): the
    //  entry `digest:common=crypto_common` of crates, `:common=crypto_common` for the root of the source (a bin's).
    //  As in rustc's extern prelude, every module of that crate sees the name, after its own names; the crate keeps its
    //  own name everywhere. (the crate's root: its module, or usize::MAX, the name) → the crate's module)
    let crate_root = |m: usize| -> usize { if in_crate(m) { mod_of_path.get(&vec![mods[m].0[0].clone()]).copied().unwrap_or(usize::MAX) } else { usize::MAX } };
    let mut ext_alias: LMap<(usize, Vec<u8>), usize> = LMap::default();
    for e in crates {
        let Some((owner, rest)) = e.split_once(':') else { continue };
        let Some((alias, target)) = rest.split_once('=') else { continue };
        let root = if owner.is_empty() { Some(usize::MAX) } else { mod_of_path.get(&vec![owner.as_bytes().to_vec()]).copied() };
        if let (Some(root), Some(&t)) = (root, mod_of_path.get(&vec![target.as_bytes().to_vec()])) {
            ext_alias.insert((root, alias.as_bytes().to_vec()), t);
        }
    }
    // (the module path a path from the extern prelude of the crate of scope `from` names: `common::block` →
    //  `crypto_common::block`)
    let ext_path = |from: usize, segs: &[Vec<u8>]| -> Option<Vec<Vec<u8>>> {
        if ext_alias.is_empty() {
            return None;
        }
        let t = *ext_alias.get(&(crate_root(from), segs.first()?.clone()))?;
        let mut p = mods[t].0.clone();
        p.extend(segs[1..].iter().cloned());
        Some(p)
    };
    // モジュールのパス (`crate::a::b` / `a::b` / `self` / `super`) → mods の番号
    let resolve_mod = |from: usize, segs: &[Vec<u8>]| -> Option<usize> {
        let mut base: Vec<Vec<u8>> = if from == usize::MAX { vec![] } else { mods[from].0.clone() };
        // (クレートのモジュールの中の `crate::` はそのモジュール)
        let croot: Vec<Vec<u8>> = if in_crate(from) { vec![mods[from].0[0].clone()] } else { vec![] };
        let mut rest = segs;
        if rest.first().is_some_and(|x| x.as_slice() == b"crate") {
            base = croot.clone();
            rest = &rest[1..];
        } else if rest.first().is_some_and(|x| x.as_slice() == b"self") {
            rest = &rest[1..];
        } else {
            while rest.first().is_some_and(|x| x.as_slice() == b"super") {
                base.pop();
                rest = &rest[1..];
            }
            // (相対パスで見つからなければ crate からとみなす)
            let mut rel = base.clone();
            rel.extend(rest.iter().cloned());
            let hit = mod_of_path.get(&rel).copied();
            // (a name of the crate's extern prelude, unless the scope binds the first name in the type namespace itself:
            // a module of that name (whose re-exports the path may go on through, `common::inner` with `pub use
            // crate::local::inner;` in it), or a type. Another crate's module of the name, which the root of the source
            // holds, is none of the crate's; a fn of the name is a value)
            if !ext_alias.is_empty() && rest.len() == segs.len() {
                let own = |m: usize| !(crate_root(from) == usize::MAX && in_crate(m));
                let mut first = base.clone();
                first.push(segs[0].clone());
                let local = mod_of_path.get(&first).copied().is_some_and(own) || ty_defs.contains(&(from, segs[0].clone()));
                if !local {
                    if let Some(p) = ext_path(from, segs) {
                        return mod_of_path.get(&p).copied();
                    }
                }
            }
            if let Some(i) = hit {
                return Some(i);
            }
            if !croot.is_empty() {
                let mut rel = croot.clone();
                rel.extend(rest.iter().cloned());
                if let Some(i) = mod_of_path.get(&rel).copied() {
                    return Some(i);
                }
            }
            base = vec![];
        }
        base.extend(rest.iter().cloned());
        if base.is_empty() {
            return Some(usize::MAX);
        }
        mod_of_path.get(&base).copied()
    };
    // (a path from `::` (`::common::block`, edition 2018 on): the name of the crate's extern prelude, else a crate's
    // module; never a module or alias of the scope's. Then the crate root's module, as edition 2015 reads it)
    let resolve_lead = |from: usize, segs: &[Vec<u8>]| -> Option<usize> {
        match ext_path(from, segs) {
            Some(p) => mod_of_path.get(&p).copied(),
            // (the crate's module by its own name: not through the names the root of the source (a bin) gives crates,
            // which are the bin's alone)
            None => mod_of_path.get(segs).copied().or_else(|| {
                let mut p = vec![b"crate".to_vec()];
                p.extend(segs.iter().cloned());
                resolve_mod(from, &p).filter(|_| in_crate(from))
            }),
        }
    };
    // (whether the `use` at token k starts from `::`, `use ::common::make;`)
    let use_lead = |k: usize| k + 2 < toks.len() && is_sym(k + 1, b':') && is_sym(k + 2, b':');
    // (a module imported by `use`, under its own name or another: `use util as u;` with the crate `util`, or
    // `use crate::geom::shape;`. (scope, the name used) → the module, so `u::calls()` is `util::calls()`)
    let mut mod_alias: LMap<(usize, Vec<u8>), usize> = LMap::default();
    // (those written `pub use`: a glob of the module brings them)
    let mut alias_pub: Vec<(usize, Vec<u8>)> = vec![];
    // (a `use` in a block (a fn body) names the module in that block only: (its `{`, its `}`, the name, the module))
    let mut block_alias: Vec<(usize, usize, Vec<u8>, usize, usize)> = vec![];
    // (the innermost `{` around each token, and the `}` of each `{`)
    let mut block_of: Vec<usize> = vec![usize::MAX; toks.len()];
    let mut close_at: LMap<usize, usize> = LMap::default();
    let mut open_st: Vec<usize> = vec![];
    for (k, b) in block_of.iter_mut().enumerate() {
        *b = open_st.last().copied().unwrap_or(usize::MAX);
        if is_sym(k, b'{') {
            open_st.push(k);
        } else if is_sym(k, b'}') {
            if let Some(o) = open_st.pop() {
                close_at.insert(o, k);
            }
        }
    }
    let mut k = 0;
    while k < toks.len() {
        if !is_id(k, b"use") {
            k += 1;
            continue;
        }
        let sc = scope_of(k);
        let mut e = k;
        while e < toks.len() && !is_sym(e, b';') {
            e += 1;
        }
        let mut items = vec![];
        use_walk(&toks, s, k + 1, e, vec![], &mut items);
        // (in the module's own braces (or at the root), or in a block inside them)
        let own = if sc == usize::MAX { usize::MAX } else { mods[sc].1 };
        let block = block_of[k];
        let lead = use_lead(k);
        for (path, local) in items {
            if local.as_slice() != b"*" && local.as_slice() != b"_" && !path.is_empty() {
                if let Some(m) = (if lead { resolve_lead(sc, &path) } else { resolve_mod(sc, &path) }).filter(|&m| m != usize::MAX) {
                    match close_at.get(&block) {
                        // (and its module: a `mod` declared in the block does not see it)
                        Some(&close) if block != own => block_alias.push((block, close, local, m, sc)),
                        _ => {
                            if pub_before(&toks, s, k) {
                                alias_pub.push((sc, local.clone()));
                            }
                            mod_alias.insert((sc, local), m);
                        }
                    }
                }
            }
        }
        k = e + 1;
    }
    // (a path at token `at` in scope `from`: its first segment through the aliases, the innermost block's first)
    // (a `mod x { .. }` declared in a block (a fn body): (the block's `{`, its `}`, the name, the module, the scope it is
    // declared in). Inside the block it shadows an alias `x` of the module's)
    let mut block_mods: Vec<(usize, usize, Vec<u8>, usize, usize)> = vec![];
    for (m, md) in mods.iter().enumerate() {
        if md.1 < 2 {
            continue;
        }
        let d = md.1 - 2;
        let sc = scope_of(d);
        let own = if sc == usize::MAX { usize::MAX } else { mods[sc].1 };
        let b = block_of[d];
        if let (Some(&c), Some(n)) = (close_at.get(&b), md.0.last()) {
            if b != own {
                block_mods.push((b, c, n.clone(), m, sc));
            }
        }
    }
    // (lead: the path is `::a::b`, whose first name is a crate's (the crate root's in 2015), not an alias a block
    // or a module brings in)
    let resolve_mod_at = |from: usize, segs: &[Vec<u8>], at: usize, lead: bool| -> Option<usize> {
        if lead {
            return resolve_lead(from, segs);
        }
        let alias = segs.first().and_then(|f| {
            // (the innermost block's alias or module of that name, then the module's alias)
            block_alias
                .iter()
                .chain(block_mods.iter())
                .filter(|b| b.0 < at && at < b.1 && b.2 == *f && b.4 == from)
                .max_by_key(|b| b.0)
                .map(|b| b.3)
                .or_else(|| mod_alias.get(&(from, f.clone())).copied())
        });
        if let Some(m) = alias {
            let mut p = mods[m].0.clone();
            p.extend(segs[1..].iter().cloned());
            return if segs.len() == 1 { Some(m) } else { mod_of_path.get(&p).copied() };
        }
        resolve_mod(from, segs)
    };
    let renamed_in = |m: usize, n: &[u8]| -> Option<Vec<u8>> { renamed_map.get(&(m, n.to_vec())).cloned() };
    // use 文で取り込んだ名前: (スコープ, 使う名前) → 新しい名前。use 文のトークン範囲は書き換えない
    let mut imports: Vec<(usize, Vec<u8>, Vec<u8>)> = vec![];
    let mut use_ranges: Vec<(usize, usize)> = vec![];
    // `use crate::m::*;` で取り込んだモジュール: (スコープ, モジュール)
    let mut globs: Vec<(usize, usize)> = vec![];
    // (モジュール m の中で use した名前を取り込むもの: (スコープ, 使う名前, m, m の中の名前, ブロック))
    let mut pending: Vec<(usize, Vec<u8>, usize, Vec<u8>, Option<(usize, usize)>, u8)> = vec![];
    // (a name a `use` in a block (a fn body) imports, seen in that block only: (its `{`, its `}`, the name used, the new
    // name). The innermost block's wins over the module's items and imports)
    let mut block_imports: Vec<(usize, usize, Vec<u8>, Vec<u8>, usize, u8)> = vec![];
    // (the namespaces of each definition: a fn / const / static is a value, an enum / trait / type alias a type, a
    //  struct / union both (a tuple struct's constructor); one defined twice under a name, both)
    let mut def_ns: LMap<(usize, Vec<u8>), u8> = LMap::default();
    for d in &defs {
        let kw = if d.2 >= 2 && is_id(d.2 - 1, b"mut") { d.2 - 2 } else { d.2.saturating_sub(1) };
        let ns = if is_id(kw, b"fn") || is_id(kw, b"const") || is_id(kw, b"static") {
            NS_VALUE
        } else if is_id(kw, b"enum") || is_id(kw, b"trait") || is_id(kw, b"type") || is_id(kw, b"union") {
            NS_TYPE
        } else if is_id(kw, b"struct") {
            // (a braced struct is a type only; a tuple or unit struct's name is its constructor too)
            let mut q = d.2 + 1;
            if q < toks.len() && is_sym(q, b'<') {
                let mut depth = 0i32;
                while q < toks.len() {
                    if is_sym(q, b'<') {
                        depth += 1;
                    } else if is_sym(q, b'>') {
                        depth -= 1;
                        if depth == 0 {
                            q += 1;
                            break;
                        }
                    }
                    q += 1;
                }
            }
            while q < toks.len() && !is_sym(q, b'{') && !is_sym(q, b'(') && !is_sym(q, b';') {
                q += 1;
            }
            if q < toks.len() && is_sym(q, b'{') { NS_TYPE } else { NS_ANY }
        } else {
            NS_ANY
        };
        *def_ns.entry((d.1, d.0.clone())).or_insert(0) |= ns;
    }
    // (what is written `pub`: modules, items, and the aliases, globs and imports below)
    let mod_pub: Vec<bool> = mods.iter().map(|m| m.1 >= 2 && pub_before(&toks, s, m.1 - 2)).collect();
    let mut vis = Vis::default();
    for d in &defs {
        let kw = if d.2 >= 2 && is_id(d.2 - 1, b"mut") { d.2 - 2 } else { d.2.saturating_sub(1) };
        if pub_before(&toks, s, kw) {
            vis.defs.insert((d.1, d.0.clone()));
        }
    }
    vis.aliases.extend(alias_pub.iter().cloned());
    let no_imports: LMap<(usize, Vec<u8>), (Vec<u8>, u8)> = LMap::default();
    macro_rules! names_with {
        ($g:expr, $i:expr) => {
            Names {
                mods: &mods,
                mod_pub: &mod_pub,
                vis: &vis,
                mod_of_path: &mod_of_path,
                mod_alias: &mod_alias,
                globs: $g,
                adj: {
                    let mut a: LMap<usize, Vec<(usize, usize)>> = LMap::default();
                    for &g in $g.iter() {
                        a.entry(g.0).or_default().push(g);
                    }
                    a
                },
                cycles: Default::default(),
                renamed: &renamed_map,
                def_ns: &def_ns,
                imports: $i,
                // (whether an import of one namespace's item, `name#t` / `name#v`, is there to look for)
                ns_imports: $i.keys().any(|k| k.1.ends_with(b"#t") || k.1.ends_with(b"#v")),
                items: Default::default(),
                modules: Default::default(),
                active_items: Default::default(),
                active_mods: Default::default(),
            }
        };
    }
    // (the module of a `use` path, also through modules that globs bring in: in libc, `pub use new::*;` at the root,
    //  `pub(crate) use linux_uapi::*;` in new, and then `pub use crate::linux::can::*;` and, in new,
    //  `pub use linux::types::*;`. The longest prefix that names a module, then the rest segment by segment; from the
    //  scope itself, the crate root too, when none does. `lead`: the path starts from `::`, not from the scope)
    let resolve_path = |sc: usize, path: &[Vec<u8>], at: usize, names: &Names, lead: bool| -> Option<usize> {
        if let Some(m) = resolve_mod_at(sc, path, at, lead) {
            return Some(m);
        }
        for cut in (0..path.len()).rev() {
            let start = if cut == 0 { Some(sc).filter(|_| !lead) } else { resolve_mod_at(sc, &path[..cut], at, lead) };
            if let Some(m) = start.and_then(|st| names.walk(sc, st, &path[cut..])) {
                return Some(m);
            }
        }
        None
    };
    // (the globs first, until no more resolve: a `use` may name a module that a later glob brings in)
    let mut glob_uses: Vec<(usize, usize, Vec<Vec<u8>>, bool, bool)> = vec![];
    {
        let mut k = 0;
        while k < toks.len() {
            if is_id(k, b"use") {
                let mut e = k;
                while e < toks.len() && !is_sym(e, b';') {
                    e += 1;
                }
                let mut items = vec![];
                use_walk(&toks, s, k + 1, e, vec![], &mut items);
                for (path, local) in items {
                    if local.as_slice() == b"*" {
                        glob_uses.push((scope_of(k), k, path, pub_before(&toks, s, k), use_lead(k)));
                    }
                }
                k = e;
            }
            k += 1;
        }
    }
    let mut done = vec![false; glob_uses.len()];
    loop {
        let mut added = vec![];
        {
            let names = names_with!(&globs, &no_imports);
            for (i, (sc, at, path, _, lead)) in glob_uses.iter().enumerate() {
                if done[i] {
                    continue;
                }
                if let Some(m) = resolve_path(*sc, path, *at, &names, *lead) {
                    done[i] = true;
                    added.push(((*sc, m), glob_uses[i].3));
                }
            }
        }
        let before = (globs.len(), vis.globs.len());
        for (g, is_pub) in added {
            if is_pub {
                vis.globs.insert(g);
            }
            if !globs.contains(&g) {
                globs.push(g);
            }
        }
        // (a glob made visible by this pass counts too: a later one may go through it)
        if (globs.len(), vis.globs.len()) == before {
            break;
        }
    }
    // (the namespaces of the names `use` imports: (scope, name) -> namespaces)
    let mut import_ns: LMap<(usize, Vec<u8>), u8> = LMap::default();
    let mut import_pub: Vec<(usize, Vec<u8>)> = vec![];
    let final_globs = globs.clone();
    let use_names = names_with!(&final_globs, &no_imports);
    let mut k = 0;
    while k < toks.len() {
        if !is_id(k, b"use") {
            k += 1;
            continue;
        }
        let sc = scope_of(k);
        let mut e = k;
        while e < toks.len() && !is_sym(e, b';') {
            e += 1;
        }
        use_ranges.push((k, e));
        let mut items = vec![];
        let mut selfs = vec![];
        use_walk_in(&toks, s, k + 1, e, vec![], &mut items, &mut selfs);
        // (a `pub use`: a glob of this module brings what it imports)
        if pub_before(&toks, s, k) {
            import_pub.extend(items.iter().filter(|it| it.1.as_slice() != b"*").map(|it| (sc, it.1.clone())));
        }
        let own_brace = if sc == usize::MAX { usize::MAX } else { mods[sc].1 };
        let in_block = close_at.get(&block_of[k]).filter(|_| block_of[k] != own_brace).map(|&c| (block_of[k], c));
        let mut add = |local: Vec<u8>, nn: Vec<u8>, ns: u8| match in_block {
            Some((o, c)) => block_imports.push((o, c, local, nn, sc, ns)),
            None => {
                import_ns.insert((sc, local.clone()), ns);
                imports.push((sc, local, nn));
            }
        };
        for (ix, (path, local)) in items.into_iter().enumerate() {
            // (a `self` entry, `use crate::a::m::{self};`, brings m's type namespace only: the module (followed as a
            //  module alias) or a type, not a fn m of a beside it)
            let want = if selfs.contains(&ix) { NS_TYPE } else { NS_ANY };
            if local.as_slice() == b"*" {
                // (resolved before this loop, to a fixed point)
                continue;
            }
            if path.len() < 2 {
                continue;
            }
            let (mpath, name) = path.split_at(path.len() - 1);
            // (an item of std under another name, `use std::cell::LazyCell as Lazy;`: the name is its path. A macro
            // too, `pub use std::println as say;`: what `m::say!(..)` names, a macro only beside the module's own
            // `fn say`. Not in a block that way: there its bare `say()` would take the import)
            let own = def_set.contains(&(sc, local.clone()));
            if matches!(mpath[0].as_slice(), b"std" | b"core" | b"alloc") && local != name[0] && local.as_slice() != b"_" && !(own && in_block.is_some()) {
                let mut full = vec![];
                for (i, seg) in path.iter().enumerate() {
                    if i > 0 {
                        full.extend_from_slice(b"::");
                    }
                    full.extend_from_slice(seg);
                }
                // (a `self` entry, `use std::io::{self as sio};`, the module only: no macro)
                let ns = if own { NS_MACRO } else { NS_ANY | NS_MACRO };
                let ns = if want == NS_TYPE { ns & NS_TYPE } else { ns };
                if ns != 0 {
                    add(local, full, ns);
                }
                continue;
            }
            // (none when the name is only a value, beside the module a `self` entry names)
            let found = resolve_path(sc, mpath, k, &use_names, use_lead(k)).map(|m| (m, def_ns.get(&(m, name[0].clone())).copied().unwrap_or(NS_ANY) & want));
            if let Some((m, ns)) = found.filter(|f| f.1 != 0) {
                if let Some(nn) = renamed_in(m, &name[0]) {
                    // (and the item of the other namespace the name has there: libc's `fn sigaction` beside the
                    //  `struct sigaction` a glob brings, under `name#t` / `name#v`, which a lookup of that namespace reads)
                    //  (not for a `self` entry: the module only)
                    let second = if want == NS_ANY { other_ns_item(&use_names, m, &name[0], ns, &nn) } else { None };
                    if let Some((n2, other)) = second {
                        add(ns_key(&local, other), n2, other);
                    }
                    add(local, nn, ns);
                } else if !def_set.contains(&(m, name[0].clone())) {
                    // (`pub use` で取り込み直した名前かもしれない: 後で解く)
                    pending.push((sc, local, m, name[0].clone(), in_block, want));
                } else if local.as_slice() != name[0].as_slice() && def_set.contains(&(m, name[0].clone())) {
                    // (`use crate::parse::{FnSig as PSig}`: 別名はそのモジュールの中で元の名前に)
                    add(local, name[0].clone(), ns);
                }
            }
        }
        k = e + 1;
    }
    drop(use_names);
    vis.imports.extend(import_pub.iter().cloned());
    // (再輸出 `pub use crate::shape::Shape;` を辿る: a name module p.2 imports by name, or one its globs pass on, globs
    //  of globs too (libc's prelude `use crate::{size_t};` with the root's `pub use unix::*;`). Until no more resolve)
    loop {
        let mut map: LMap<(usize, Vec<u8>), (Vec<u8>, u8)> = LMap::default();
        for i in &imports {
            let key = (i.0, i.1.clone());
            let ns = import_ns.get(&key).copied().unwrap_or(NS_ANY);
            map.entry(key).or_insert((i.2.clone(), ns));
        }
        let mut found_any = vec![];
        let mut rest = vec![];
        // (the other namespace's items of the names found, other_ns_item: (scope, key, item, namespace))
        let mut seconds = vec![];
        {
            let names = names_with!(&final_globs, &map);
            for p in std::mem::take(&mut pending) {
                // (as module p.2 sees it: what its globs bring is what it imports)
                match names.item_at(p.2, p.2, &p.3, p.5) {
                    GlobHit::Item(nn, ns) => {
                        // (not for a `self` entry, which is the module (the type namespace) only)
                        if p.5 == NS_ANY && p.4.is_none() {
                            if let Some((n2, other)) = other_ns_item(&names, p.2, &p.3, ns, &nn) {
                                seconds.push((p.0, ns_key(&p.1, other), n2, other));
                            }
                        }
                        found_any.push((p, nn, ns))
                    }
                    _ => rest.push(p),
                }
            }
        }
        pending = rest;
        if found_any.is_empty() {
            break;
        }
        for (sc, k2, n2, other) in seconds {
            import_ns.insert((sc, k2.clone()), other);
            imports.push((sc, k2, n2));
        }
        for (p, nn, ns) in found_any {
            let ns = ns & p.5;
            match p.4 {
                Some((o, c)) => block_imports.push((o, c, p.1, nn, p.0, ns)),
                None => {
                    import_ns.insert((p.0, p.1.clone()), ns);
                    imports.push((p.0, p.1, nn));
                }
            }
        }
    }
    // enum のバリアント名の位置 (`enum E { Ty(Ty), A }` の `Ty` / `A`) は名前ではないので変えない
    let mut variant_toks: Vec<usize> = vec![];
    let mut k = 0;
    while k + 1 < toks.len() {
        if is_id(k, b"enum") && toks[k + 1].0 == 0 {
            let mut j = k + 2;
            while j < toks.len() && !is_sym(j, b'{') && !is_sym(j, b';') {
                j += 1;
            }
            if j < toks.len() && is_sym(j, b'{') {
                let mut d = 0i32;
                let mut expect = true;
                let mut m = j;
                while m < toks.len() {
                    if is_sym(m, b'{') || is_sym(m, b'(') || is_sym(m, b'[') {
                        d += 1;
                    } else if is_sym(m, b'}') || is_sym(m, b')') || is_sym(m, b']') {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    } else if d == 1 {
                        if is_sym(m, b',') {
                            expect = true;
                        } else if is_sym(m, b'#') {
                            // 属性はそのまま
                        } else if expect && toks[m].0 == 0 {
                            variant_toks.push(m);
                            expect = false;
                        }
                    }
                    m += 1;
                }
            }
        }
        k += 1;
    }
    // 書き換え
    // (飛ばすトークン (use 文・バリアント名)、定義の名前のトークン、取り込んだ名前の表 (先のものが勝つ))
    let mut skip: Vec<bool> = vec![false; toks.len()];
    for &(a, b) in &use_ranges {
        let mut k = a;
        while k < (b + 1).min(toks.len()) {
            skip[k] = true;
            k += 1;
        }
    }
    let mut variant_tok: Vec<bool> = vec![false; toks.len()];
    for &v in &variant_toks {
        skip[v] = true;
        variant_tok[v] = true;
    }
    // (nor an attribute's tokens or a macro's name: `#[derive(Eq)]` / `write!(..)` beside a module's `type Eq` /
    // `fn write`. The modules of a macro's path are a path's, `m::say!(..)`'s m)
    let macro_ns = macro_ns_toks(&toks, s);
    for (sk, &m) in skip.iter_mut().zip(&macro_ns) {
        *sk |= m;
    }
    let mut def_tok: Vec<bool> = vec![false; toks.len()];
    for d in &defs {
        def_tok[d.2] = true;
    }
    let mut imp_map: LMap<(usize, Vec<u8>), (Vec<u8>, u8)> = LMap::default();
    for i in &imports {
        let key = (i.0, i.1.clone());
        let ns = import_ns.get(&key).copied().unwrap_or(NS_ANY);
        imp_map.entry(key).or_insert((i.2.clone(), ns));
    }
    // (変える名前・取り込んだ名前のハッシュ: 当たらない名前は表を引かない。偶然当たっても表を引くだけ)
    let mut interest: LSet<u64> = LSet::default();
    for r in &renamed {
        interest.insert(name_hash(&r.1));
    }
    for i in &imports {
        interest.insert(name_hash(&i.1));
    }
    for i in &block_imports {
        interest.insert(name_hash(&i.2));
    }
    // (a name a fn binds itself, a parameter `guard: Self` or a `let guard = ..`, is that local in the fn's body, not
    // the item of that name: `fn into_inner(guard: Self) { ManuallyDrop::new(guard) }` next to `pub fn guard`)
    let mut local_tok: Vec<bool> = vec![false; toks.len()];
    {
        let mut close: Vec<usize> = vec![usize::MAX; toks.len()];
        let mut st: Vec<usize> = vec![];
        // (and where the `<`s and the `fn`s are: the loops below start only there)
        let mut lts: Vec<usize> = vec![];
        let mut fns: Vec<usize> = vec![];
        for (k, t) in toks.iter().enumerate() {
            match t.0 {
                1 if t.2 == t.1 + 1 => match s[t.1] {
                    b'(' | b'{' | b'[' => st.push(k),
                    b')' | b'}' | b']' => {
                        if let Some(o) = st.pop() {
                            close[o] = k;
                        }
                    }
                    b'<' => lts.push(k),
                    _ => {}
                },
                0 if t.2 == t.1 + 2 && &s[t.1..t.2] == b"fn" => fns.push(k),
                _ => {}
            }
        }
        // (and the names of std items under another name: `let release = 1;` beside `use std::mem::drop as release;`
        // is the local)
        let std_path = |nn: &[u8]| [&b"std::"[..], b"core::", b"alloc::"].iter().any(|p| nn.starts_with(p));
        let renamed_names: Vec<&[u8]> = renamed
            .iter()
            .map(|r| r.1.as_slice())
            .chain(imports.iter().filter(|i| std_path(&i.2)).map(|i| i.1.as_slice()))
            .chain(block_imports.iter().filter(|i| std_path(&i.3)).map(|i| i.2.as_slice()))
            .collect();
        // (the names a generic parameter list `<..>` at g declares, of those, and the index of its `>`: `<C, const N:
        // usize, T: Tr<C>>` declares C, N and T)
        let generics = |g: usize| -> (Vec<&[u8]>, usize) {
            let mut names = vec![];
            let mut d = 0usize;
            let mut q = g;
            while q < toks.len() {
                if is_sym(q, b'<') {
                    d += 1;
                } else if is_sym(q, b'>') && !(q >= 1 && is_sym(q - 1, b'-')) {
                    d -= 1;
                    if d == 0 {
                        return (names, q);
                    }
                } else if (is_sym(q, b'(') || is_sym(q, b'[') || is_sym(q, b'{')) && close[q] != usize::MAX {
                    // (`const B: bool = { 2 > 1 }`: its `>` is no end)
                    q = close[q];
                } else if d == 1 && toks[q].0 == 0 && (is_sym(q - 1, b'<') || is_sym(q - 1, b',') || is_id(q - 1, b"const")) && renamed_names.contains(&tok(q)) {
                    names.push(tok(q));
                }
                q += 1;
            }
            (names, toks.len() - 1)
        };
        // (a generic parameter in its scope, where it is no later path segment: `C`, `x: C`, `C: Tr`, and a path's root,
        // `C::default()`; not `a::C`)
        let local_at = |q: usize| toks[q].0 == 0 && !(q >= 2 && is_sym(q - 1, b':') && is_sym(q - 2, b':'));
        // (an item's generic parameters, in the whole item: `impl<C> X<C> { .. }`, `struct S<C>(C);`)
        // (from each `<`: an item's when `impl` or `struct S` / `enum E` / .. is before it)
        for &g in lts.iter().filter(|_| !renamed_names.is_empty()) {
            let k = g;
            if k == 0 || k + 1 >= toks.len() {
                continue;
            }
            let item = is_id(k - 1, b"impl")
                || (k >= 2 && toks[k - 1].0 == 0 && [&b"struct"[..], b"enum", b"union", b"trait", b"type"].iter().any(|w| is_id(k - 2, w)));
            if !item {
                continue;
            }
            let (names, ge) = generics(g);
            if !names.is_empty() {
                let mut e = ge + 1;
                while e < toks.len() && !is_sym(e, b'{') && !is_sym(e, b';') {
                    if (is_sym(e, b'(') || is_sym(e, b'[')) && close[e] != usize::MAX {
                        e = close[e];
                    }
                    e += 1;
                }
                let until = if e < toks.len() && is_sym(e, b'{') && close[e] != usize::MAX { close[e] } else { e.min(toks.len() - 1) };
                for q in g + 1..until {
                    if names.contains(&tok(q)) && local_at(q) {
                        local_tok[q] = true;
                    }
                }
            }
        }
        // (no name to bind: nothing for a function to make local)
        let mut fi = if renamed_names.is_empty() { fns.len() } else { 0 };
        let mut k = 0;
        while fi < fns.len() {
            // (the next `fn` at or after k: one inside a body that was read goes too, as before)
            if fns[fi] < k {
                fi += 1;
                continue;
            }
            k = fns[fi];
            fi += 1;
            if k + 2 >= toks.len() || toks[k + 1].0 != 0 {
                continue;
            }
            // (each name and where it is the local: a generic parameter or a parameter in the whole function, a `let`
            // after its statement to the end of its block)
            let mut bound: Vec<(&[u8], usize, usize)> = vec![];
            // (the parameters: `(`, after the generics; a generic parameter is a path's root too, `C::default()`)
            let mut p = k + 2;
            let mut generic_names: Vec<&[u8]> = vec![];
            if is_sym(p, b'<') {
                let (names, ge) = generics(p);
                for n in names {
                    bound.push((n, p + 1, usize::MAX));
                    generic_names.push(n);
                }
                p = ge + 1;
            }
            if p >= toks.len() || !is_sym(p, b'(') || close[p] == usize::MAX {
                k += 1;
                continue;
            }
            let pe = close[p];
            for q in p + 1..pe {
                if toks[q].0 == 0 && q + 2 <= pe && is_sym(q + 1, b':') && !is_sym(q + 2, b':') && renamed_names.contains(&tok(q)) {
                    bound.push((tok(q), p + 1, usize::MAX));
                }
            }
            // (the body: the first `{` after the parameters, or none (`;`: then the signature's end))
            let mut b = pe + 1;
            while b < toks.len() && !is_sym(b, b'{') && !is_sym(b, b';') {
                b += 1;
            }
            let body = b < toks.len() && is_sym(b, b'{') && close[b] != usize::MAX;
            let be = if body { close[b] } else { b.min(toks.len() - 1) };
            for q in (b + 1..be).filter(|_| body) {
                if is_id(q, b"let") {
                    let n = if q + 1 < be && is_id(q + 1, b"mut") { q + 2 } else { q + 1 };
                    // (a plain name: `let FnvHasher(mut h) = ..` is a pattern of the struct)
                    let plain = n + 1 < be && (is_sym(n + 1, b'=') || is_sym(n + 1, b';') || (is_sym(n + 1, b':') && !is_sym(n + 2, b':')));
                    if n < be && toks[n].0 == 0 && plain && renamed_names.contains(&tok(n)) {
                        // (the end of the innermost block around the `let`)
                        let mut d = 0usize;
                        let mut o = q;
                        let mut until = be;
                        while o > b {
                            o -= 1;
                            if is_sym(o, b'}') {
                                d += 1;
                            } else if is_sym(o, b'{') {
                                if d == 0 {
                                    until = close[o];
                                    break;
                                }
                                d -= 1;
                            }
                        }
                        // (the name itself, then after its statement: `let release = release(1);` calls the item. An
                        // `if let` / `while let` binds in its body only: from its `{`, not in its scrutinee)
                        let cond = q > b + 1 && (is_id(q - 1, b"if") || is_id(q - 1, b"while") || is_sym(q - 1, b'&'));
                        // (the scrutinee's own blocks are not the body: `= { 3 } {`, after an operator, and the block of
                        // a `match` / `if` / `loop` .. in it)
                        let mut pend = 0usize;
                        let mut j = n + 1;
                        while j < be && !is_sym(j, b';') {
                            if cond && is_sym(j, b'{') {
                                let after_op = toks[j - 1].0 == 1 && b"=+-*/%&|^!<>,([".contains(&s[toks[j - 1].1]);
                                if (pend > 0 || after_op) && close[j] != usize::MAX {
                                    pend = pend.saturating_sub(1);
                                    j = close[j] + 1;
                                    continue;
                                }
                                break;
                            }
                            if cond && toks[j].0 == 0 && matches!(tok(j), b"match" | b"if" | b"while" | b"for" | b"loop" | b"unsafe" | b"async" | b"else") {
                                pend += 1;
                            }
                            if (is_sym(j, b'(') || is_sym(j, b'[') || (!cond && is_sym(j, b'{'))) && close[j] != usize::MAX {
                                j = close[j];
                            }
                            j += 1;
                        }
                        let until = if cond && j < be && is_sym(j, b'{') && close[j] != usize::MAX { close[j] } else { until };
                        bound.push((tok(n), n, n));
                        bound.push((tok(n), j, until));
                    }
                }
            }
            if !bound.is_empty() {
                let is_bound = |q: usize| bound.iter().any(|&(nm, from, until)| nm == tok(q) && q >= from && q <= until);
                // (only a type comes after `*mut` / `*const` and `->`: `fn mktime(tm: *mut tm)` names the struct tm there,
                //  not the parameter)
                let type_only = |q: usize| {
                    q >= 2 && (is_id(q - 1, b"mut") || is_id(q - 1, b"const")) && is_sym(q - 2, b'*')
                        || q >= 2 && is_sym(q - 1, b'>') && is_sym(q - 2, b'-')
                        // (`fn f(t: &t)` / `&'a t` / `&mut t`: a reference type after a `:`)
                        || (type_place_before(&toks, s, q) && !is_sym(q - 1, b':') && !is_sym(q - 1, b'<') && !is_id(q - 1, b"as"))
                };
                for q in k + 2..be {
                    // (a generic parameter is a type: it stays the fn's there too, `fn id<C>(x: C) -> C`)
                    if type_only(q) && !generic_names.contains(&tok(q)) {
                        continue;
                    }
                    if toks[q].0 == 0 && is_bound(q) && !(q >= 2 && is_sym(q - 1, b':') && is_sym(q - 2, b':')) && !is_sym(q + 1, b':')
                        || (toks[q].0 == 0 && is_bound(q) && q + 2 <= be && is_sym(q + 1, b':') && !is_sym(q + 2, b':'))
                        || (generic_names.contains(&tok(q)) && is_bound(q) && local_at(q))
                    {
                        local_tok[q] = true;
                    }
                }
            }
            k = b;
        }
    }
    // (whether token k is directly in an impl / trait body: its innermost `{` opens one, `impl .. {` / `trait .. {`)
    let assoc_body: Vec<bool> = {
        let mut out = vec![false; toks.len()];
        let mut stack: Vec<bool> = vec![];
        for k in 0..toks.len() {
            if is_sym(k, b'{') {
                // (the item's head: back to the last `;` / `{` / `}`)
                let mut j = k;
                let (mut assoc, mut is_fn) = (false, false);
                // (inside `<..>` of the head, a brace group is a const argument (`impl T for S<{ 1 }> {`): skipped)
                let mut angle = 0i32;
                while j > 0 {
                    j -= 1;
                    // (the `>` of `->` / `=>` is none)
                    let arrow = j > 0 && (is_sym(j - 1, b'-') || is_sym(j - 1, b'=')) && toks[j - 1].2 == toks[j].1;
                    if is_sym(j, b'>') && !arrow {
                        angle += 1;
                    } else if is_sym(j, b'<') {
                        angle -= 1;
                    } else if angle > 0 && is_sym(j, b'}') {
                        let mut d = 1;
                        while j > 0 && d > 0 {
                            j -= 1;
                            if is_sym(j, b'}') {
                                d += 1;
                            } else if is_sym(j, b'{') {
                                d -= 1;
                            }
                        }
                        continue;
                    }
                    if is_sym(j, b';') || is_sym(j, b'{') || is_sym(j, b'}') {
                        break;
                    }
                    // (`fn f() -> impl Iterator {` is a fn's body, not an impl's; a fn pointer type (`impl T for fn() {`)
                    // is no fn: a declaration names one, `fn f`)
                    is_fn |= is_id(j, b"fn") && j + 1 < k && toks[j + 1].0 == 0;
                    assoc |= is_id(j, b"impl") || is_id(j, b"trait");
                }
                stack.push(assoc && !is_fn);
            } else if is_sym(k, b'}') {
                stack.pop();
            } else {
                out[k] = stack.last().copied().unwrap_or(false);
            }
        }
        out
    };
    let mut repl: Vec<(usize, usize, Vec<u8>)> = vec![]; // (開始トークン, 終了トークン (含む), 新しいテキスト)
    // (the names as the rewrite sees them: through globs, by namespace, with what `use` imports)
    let names = names_with!(&final_globs, &imp_map);
    let mut k = 0;
    while k < toks.len() {
        if toks[k].0 != 0 || skip[k] || local_tok[k] {
            k += 1;
            continue;
        }
        // (a field or method after `.`; not a range's end, `a..name()`)
        let prev_dot = k >= 1 && is_sym(k - 1, b'.') && !(k >= 2 && is_sym(k - 2, b'.'));
        // (`::name` at a path's start, `::re::label` / `<T as ::re::Tr>`, is a path from the extern crate `name`: read as
        // `re::label` is)
        let leading = k >= 2
            && is_sym(k - 1, b':')
            && is_sym(k - 2, b':')
            && (k == 2
                || !(toks[k - 3].0 == 0 || is_sym(k - 3, b'>'))
                || (toks[k - 3].0 == 0
                    && matches!(tok(k - 3), b"as" | b"dyn" | b"impl" | b"for" | b"in" | b"return" | b"let" | b"mut" | b"ref" | b"box" | b"where" | b"use" | b"pub" | b"else" | b"match" | b"if" | b"while" | b"move" | b"break" | b"yield" | b"const" | b"static" | b"unsafe")));
        let prev_path = k >= 2 && is_sym(k - 1, b':') && is_sym(k - 2, b':') && !leading;
        // (impl / trait の中の関連型の定義 `type Buffer = ..;` は、同じ名前のトップレベルの型があっても変えない)
        let assoc_def = k >= 1 && is_id(k - 1, b"type") && !def_tok[k];
        // (nor a method's `fn find(&self ..)` in an impl / trait beside a module's `fn find`: only the item is renamed.
        // A fn nested in a body is renamed with its calls, as before)
        let method_def = k >= 1 && is_id(k - 1, b"fn") && !def_tok[k] && assoc_body[k];
        if prev_dot || prev_path || assoc_def || method_def {
            k += 1;
            continue;
        }
        // パス `a::b::Name` (先頭から)
        let mut j = k;
        while j + 3 < toks.len() && is_sym(j + 1, b':') && is_sym(j + 2, b':') && toks[j + 3].0 == 0 {
            j += 3;
        }
        // (変える名前・取り込んだ名前でもなく、パスの先頭でもないなら何もしない。A number's suffix is no name: `0u8`
        //  beside fastrand's `fn u8` (#254))
        if j == k && (!interest.contains(&name_hash(tok(k))) || number_suffix(s, toks[k].1)) {
            k += 1;
            continue;
        }
        let mut segs: Vec<Vec<u8>> = vec![tok(k).to_vec()];
        let mut jj = k;
        while jj < j {
            jj += 3;
            segs.push(tok(jj).to_vec());
        }
        let sc = scope_of(k);
        // (a type's place looks only at types: `*mut stat` beside `fn stat` is the struct a glob brings. Not a path a `(`
        //  follows, after its last segment: a call, `0 < dep::f()`)
        let call = j + 1 < toks.len() && (is_sym(j + 1, b'(') || is_sym(j + 1, b'!'));
        // (a call's name is a value (a fn, a tuple struct's constructor); a type's place, a type)
        let want = if call {
            NS_VALUE
        } else if type_place_before(&toks, s, if leading { k - 2 } else { k }) {
            NS_TYPE
        } else {
            NS_ANY
        };
        if segs.len() >= 2 {
            // 接頭辞のいちばん長いモジュールから見る
            let mut done = false;
            for cut in (1..segs.len()).rev() {
                // (a module by its path, or through modules that globs bring in: `facade::api::T` with facade's
                //  `pub use crate::pkg::*;`)
                let m = resolve_mod_at(sc, &segs[..cut], k, leading)
                    .or_else(|| resolve_path(sc, &segs[..cut], k, &names, leading).filter(|&m| m != usize::MAX || segs[0].as_slice() == b"crate"));
                if let Some(m) = m {
                    if m == usize::MAX && !(segs[0].as_slice() == b"crate") {
                        continue;
                    }
                    // (its item, what it imports, or what its globs pass on, globs of globs too: libc's root
                    //  `pub use unix::*;` then unix's `pub use self::linux::*;` give `crate::gid_t` / `libc::passwd`)
                    // (a segment a path goes on after names a type or module: `m::E::V(..)`'s E is no call)
                    // (and a macro's name, the last of `m::say!(..)`, a macro: std's that m imports, never m's
                    // `fn say`)
                    let w = if cut + 1 < segs.len() {
                        NS_TYPE
                    } else if macro_ns[j] {
                        NS_MACRO
                    } else {
                        want
                    };
                    if let Some(nn) = names.lookup_from(sc, m, &segs[cut], w) {
                        // segs[..=cut] を新しい名前にする (`::re::label`: the leading `::` too, as the name is the item's)
                        repl.push((if leading { k - 2 } else { k }, k + 3 * cut, nn));
                        done = true;
                    }
                    break;
                }
            }
            if done {
                k = j + 1;
                continue;
            }
        }
        // 裸の名前 (フィールドの宣言・初期化 `name: ..` は除く)
        let next_colon = k + 1 < toks.len() && is_sym(k + 1, b':') && !(k + 2 < toks.len() && is_sym(k + 2, b':'));
        // (but a bound of a `where` clause, `where JsValue: From<T>`, is a type: back over the clause's other bounds to
        // its `where`, not past an unclosed bracket or the end of an item)
        let in_where = || {
            let mut j = k;
            let mut d = 0i32;
            let mut n = 0;
            while j > 0 && n < 400 {
                j -= 1;
                n += 1;
                if toks[j].0 == 0 && tok(j) == b"where" && d == 0 {
                    return true;
                }
                if toks[j].0 == 1 {
                    match s[toks[j].1] {
                        b')' | b']' | b'}' => d += 1,
                        b'(' | b'[' | b'{' => {
                            if d == 0 {
                                return false;
                            }
                            d -= 1;
                        }
                        b';' if d == 0 => return false,
                        _ => {}
                    }
                }
            }
            false
        };
        let is_def = def_tok[k];
        {
            let n = tok(k);
            // (a primitive type's name that is not called, `Result<u32, Error>` / `[u8]` / `u32::MAX`, is the primitive
            //  unless a type of that name shadows it: getrandom's `pub fn u32() -> Result<u32, Error>` is a value, and
            //  the `u32` beside it stays the type (#254). A call, `u32()`, is the fn, and so is a call's argument by
            //  itself, `.map_or(x, char)`: a lowercase name before the `(` calls a fn or method, whose arguments are values
            let call_arg = || {
                if !(k >= 1 && (is_sym(k - 1, b'(') || is_sym(k - 1, b',')) && k + 1 < toks.len() && (is_sym(k + 1, b')') || is_sym(k + 1, b','))) {
                    return false;
                }
                let (mut q, mut d, mut steps) = (k, 0i32, 0);
                while q > 0 && steps < 400 {
                    q -= 1;
                    steps += 1;
                    if is_sym(q, b')') || is_sym(q, b']') || is_sym(q, b'}') {
                        d += 1;
                    } else if is_sym(q, b'[') || is_sym(q, b'{') || (is_sym(q, b';') && d == 0) {
                        if d == 0 {
                            return false;
                        }
                        d -= 1;
                    } else if is_sym(q, b'(') {
                        if d == 0 {
                            // (not a fn pointer's type, `fn(u8)`, nor a tuple struct's or variant's fields,
                            //  `struct u8x2(u8, u8);` / `enum E { lower(u8) }`)
                            let fields = (q >= 2 && (is_id(q - 2, b"struct") || is_id(q - 2, b"union"))) || (q >= 1 && variant_tok[q - 1]);
                            return q >= 1 && toks[q - 1].0 == 0 && s[toks[q - 1].1].is_ascii_lowercase() && !is_id(q - 1, b"fn") && !fields;
                        }
                        d -= 1;
                    }
                }
                false
            };
            // (and so is a value in an expression's place: a `let`'s or an assignment's, `let f = u32;` (not a binding or
            //  a default among generics, `Iterator<Item = u8>` / `<T = u8>`), an operand of a comparison, `f == u32` /
            //  `u32 != f` / `f <= u32` (fn pointers compare; arithmetic on them is no Rust), an arm's, `=> u32`, and after
            //  `return` / `break`. A type's place is many more (`[u8]`, `(u8, char)`, `impl X for u8`, ..): what is
            //  neither is a type. A path's head, `n == u32::MAX`, is the type's in any place)
            // (a struct literal there, `let s = S {};` / `=> S { .. }` / `return S {}`, names the type: rustc resolves it
            //  among types. After a comparison a `{` opens the `if`'s block, `if x == B { .. }`: the operand is a value)
            let literal = k + 1 < toks.len() && is_sym(k + 1, b'{');
            let value_place = || {
                if is_sym(k - 1, b'=') {
                    // (`==` / `!=` / `<=` / `>=`, or a compound assignment: an operand)
                    let op = k >= 2 && toks[k - 2].0 == 1 && toks[k - 2].2 == toks[k - 1].1 && b"=!<>+-*/%&|^".contains(&s[toks[k - 2].1]);
                    let binding = k >= 3 && toks[k - 2].0 == 0 && (is_sym(k - 3, b'<') || is_sym(k - 3, b','));
                    return op || (!binding && !literal);
                }
                // (the left operand of `==` / `!=`; not of `<=` / `>=`, whose `>` may close generics, `Vec<u8>= ..`)
                let cmp_next = k + 2 < toks.len() && (is_sym(k + 1, b'=') || is_sym(k + 1, b'!')) && is_sym(k + 2, b'=') && toks[k + 1].2 == toks[k + 2].1;
                cmp_next || (!literal && ((k >= 2 && is_sym(k - 1, b'>') && is_sym(k - 2, b'=')) || is_id(k - 1, b"return") || is_id(k - 1, b"break")))
            };
            let prim = (j > k || !call) && is_primitive_name(n) && !(j == k && k >= 1 && want != NS_TYPE && (call_arg() || value_place()));
            // (a block's own import first: `fn f() { use b::g; g() }` beside a module-level `g`; for a primitive's name,
            //  one in the type namespace, `use crate::Byte as u8;`, as it shadows the primitive)
            // (the first segment of a path that goes on is a type or module: `E::V(..)`)
            let w = if j > k { NS_TYPE } else { want };
            // (in the place's namespaces: a block's `use crate::a::E::{self};` is the type, not the call `E()` nor the
            //  value of `let f = E;` (a value's place, as for a primitive's name above). A type's place that finds none
            //  takes any, as `lookup` does, but not for a primitive's name)
            let in_block = || {
                if is_def {
                    return None;
                }
                let of = |b: &&(usize, usize, Vec<u8>, Vec<u8>, usize, u8)| b.0 < k && k < b.1 && b.2 == n && b.4 == sc;
                if !block_imports.iter().any(|b| of(&b)) {
                    return None;
                }
                let find = |ns: u8| block_imports.iter().filter(|b| of(b) && b.5 & ns != 0).max_by_key(|b| b.0).map(|b| b.3.clone());
                if prim {
                    find(NS_TYPE)
                } else if w == NS_TYPE {
                    find(NS_TYPE).or_else(|| find(NS_ANY))
                } else if w == NS_ANY && j == k && k >= 1 && (value_place() || call_arg()) {
                    find(NS_VALUE)
                } else {
                    find(w)
                }
            };
            // (the scope's own item, its import, or what its globs pass on, globs of globs too, all of them compared
            //  (two distinct is ambiguous): libc's unix sees `time_t` of linux_like/linux/gnu/b64; a definition is its own)
            let nn = in_block().or_else(|| if is_def { renamed_in(sc, n) } else if prim { names.lookup_type(sc, n) } else { names.lookup(sc, n, w) });
            // (not a macro's name: `use core::panic as p;` then `p!()` stays as it is written)
            let std_path = |nn: &[u8]| [&b"std::"[..], b"core::", b"alloc::"].iter().any(|p| nn.starts_with(p));
            // (a name before a `:` only when it is no field: the walk back to a `where` is for a name that is renamed)
            if let Some(nn) = nn.filter(|nn| !(std_path(nn) && k + 1 < toks.len() && is_sym(k + 1, b'!'))) {
                if !next_colon || is_def || in_where() {
                    repl.push((k, k, nn));
                }
            }
        }
        k += 1;
    }
    for (sc, n, start) in &extern_defs {
        if renamed_map.contains_key(&(*sc, n.clone())) {
            let mut t = format!("#[link_name = \"{}\"] ", String::from_utf8_lossy(n)).into_bytes();
            t.extend_from_slice(tok(*start));
            repl.push((*start, *start, t));
        }
    }
    if repl.is_empty() {
        return None;
    }
    repl.sort_by_key(|r| r.0);
    let mut out = Vec::with_capacity(s.len() + repl.len() * 8);
    let mut last = 0;
    // (桁のずれ: 今の行と、その行の行頭の out の位置、ずれの合計)
    let (mut line, mut line_start, mut cum, mut scanned) = (1u32, 0usize, 0i32, 0usize);
    // (行頭からの文字数: 前に数えた所から数え足す)
    let (mut cl, mut cp, mut cc) = (usize::MAX, 0usize, 0u32);
    for (a, b, t) in &repl {
        let (st, en) = (toks[*a].1, toks[*b].2);
        if st < last {
            continue;
        }
        out.extend_from_slice(&s[last..st]);
        out.extend_from_slice(t);
        last = en;
        while scanned < out.len() {
            if out[scanned] == b'\n' {
                line += 1;
                line_start = scanned + 1;
                cum = 0;
            }
            scanned += 1;
        }
        let d = t.len() as i32 - (en - st) as i32;
        if d != 0 {
            cum += d;
            if cl != line_start {
                cl = line_start;
                cp = line_start;
                cc = 0;
            }
            cc += out[cp..].iter().filter(|&&x| (x & 0xC0) != 0x80).count() as u32;
            cp = out.len();
            let col = cc;
            fix.push((line, col, cum));
        }
    }
    out.extend_from_slice(&s[last..]);
    String::from_utf8(out).ok()
}

pub fn flatten_modules(src: &str) -> Option<String> {
    flatten_modules_in(src, &[])
}

pub fn flatten_modules_in(src: &str, crates: &[String]) -> Option<String> {
    flatten_modules_fix(src, crates, &mut vec![])
}

/// fix: rename_module_collisions_fix と同じ (平らにするのは位置を変えない)
pub fn flatten_modules_fix(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>) -> Option<String> {
    let aliased = expand_module_aliases(src);
    let changed = aliased.is_some();
    let src: &str = aliased.as_deref().unwrap_or(src);
    let r = flatten_modules_fix_in(src, crates, fix);
    if r.is_none() && changed {
        return aliased;
    }
    r
}

/// A module imported by a `use` (`use crate::arch::generic::memchr as generic;`, `use crate::arch::{generic::packedpair}`):
/// in its module (not the modules inside it), a path through the name (`generic::Iter`) is written through the
/// module's path instead (`crate::arch::generic::memchr::Iter`), which the flattening of modules renames. None if
/// there is none.
pub fn expand_module_aliases(src: &str) -> Option<String> {
    if !src.contains("use") || !src.contains("mod") {
        return None;
    }
    let s = src.as_bytes();
    let toks = tokenize(s);
    let tk = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let is_id = |k: usize| k < toks.len() && toks[k].0 == 0;
    // (modules: the names, and the token ranges of their bodies)
    let mut names: Vec<&[u8]> = vec![];
    let mut ranges: Vec<(usize, usize)> = vec![];
    // (every `{ .. }`: a `use` in a function body is that block's)
    let mut blocks: Vec<(usize, usize)> = vec![];
    let mut stack: Vec<(usize, i32)> = vec![];
    let mut open: Vec<usize> = vec![];
    let mut depth = 0i32;
    for k in 0..toks.len() {
        if is_id(k) && tk(k) == b"mod" && is_id(k + 1) && is_sym(k + 2, b'{') {
            names.push(tk(k + 1));
            stack.push((k + 2, depth + 1));
        } else if is_sym(k, b'{') {
            depth += 1;
            open.push(k);
        } else if is_sym(k, b'}') {
            if let Some(o) = open.pop() {
                blocks.push((o, k));
            }
            if stack.last().is_some_and(|&(_, d)| d == depth) {
                let (o, _) = stack.pop().unwrap();
                ranges.push((o, k));
            }
            depth -= 1;
        }
    }
    if names.is_empty() {
        return None;
    }
    // (the innermost block around token k: a module's body, or a function's for a `use` in it. The `{`s still open
    // before k, read forward as k grows (the `use`s are asked in order): the innermost of them that a `}` closes.
    // Looking through every block for each `use` was quadratic in a program of many modules)
    let close_of: LMap<usize, usize> = blocks.iter().copied().collect();
    let mut open_at: Vec<usize> = vec![];
    let mut read = 0usize;
    let mut scope = |k: usize| -> Option<(usize, usize)> {
        while read < k {
            if is_sym(read, b'{') {
                open_at.push(read);
            } else if is_sym(read, b'}') {
                open_at.pop();
            }
            read += 1;
        }
        open_at.iter().rev().find_map(|&o| close_of.get(&o).map(|&c| (o, c)))
    };
    let mut reps: Vec<(usize, Vec<u8>, usize)> = vec![]; // (token, its new text, the size of the use's scope)
    // (the tokens in the braces after a `use` tree's prefix, `use crate::{hir::x, m::Y};`: a path there goes on from
    //  the prefix, so its `m` is the prefix's child, not the module imported as `m` (regex-syntax's hir has
    //  `use crate::unicode;` and `pub use crate::{.., unicode::CaseFoldError};`, #254). Found once a name is imported)
    let mut in_tree: Vec<bool> = vec![];
    let use_trees = || {
        let mut v = vec![false; toks.len()];
        let mut k = 0;
        while k < toks.len() {
            if is_id(k) && tk(k) == b"use" && !(k >= 1 && (is_sym(k - 1, b'.') || is_sym(k - 1, b':'))) {
                // (the open groups, each whether it goes on from a prefix: `a::{..}` does, and a group in it; a group
                //  right after `use`, `use {a::q as b};`, names from the scope as the `use` itself does)
                let mut groups: Vec<bool> = vec![];
                let mut j = k + 1;
                while j < toks.len() && !is_sym(j, b';') {
                    if is_sym(j, b'{') {
                        let prefixed = groups.last().copied().unwrap_or(false) || is_sym(j - 1, b':');
                        groups.push(prefixed);
                    } else if is_sym(j, b'}') {
                        groups.pop();
                    } else if groups.last().copied().unwrap_or(false) {
                        v[j] = true;
                    }
                    j += 1;
                }
                k = j;
            }
            k += 1;
        }
        v
    };
    let mut k = 0;
    while k < toks.len() {
        // (`use a::b::m as x;`, in a tree too: `use crate::{arch::m as x, y};`)
        if !(is_id(k) && tk(k) == b"use") || (k >= 1 && (is_sym(k - 1, b'.') || is_sym(k - 1, b':'))) {
            k += 1;
            continue;
        }
        let Some(e) = (k + 1..toks.len()).find(|&j| is_sym(j, b';')) else { break };
        let mut binds = vec![];
        use_walk(&toks, s, k + 1, e, vec![], &mut binds);
        let sc = scope(k);
        let (lo, hi) = sc.map_or((0, toks.len()), |(o, c)| (o + 1, c));
        // (the modules inside it do not see the name)
        let inner: Vec<(usize, usize)> = ranges.iter().copied().filter(|&(o, c)| lo <= o && c < hi && Some((o, c)) != sc).collect();
        for (path, alias) in binds {
            let Some(last) = path.last() else { continue };
            if path.len() < 2 || alias == b"*" || alias == b"_" || !names.contains(&last.as_slice()) {
                continue;
            }
            // (from `::` as the `use` is: `use ::common::inner as i;`, `::common::inner::hi()`)
            let mut text = path.join(&b"::"[..]);
            if is_sym(k + 1, b':') && is_sym(k + 2, b':') {
                text = [&b"::"[..], &text].concat();
            }
            if in_tree.is_empty() {
                in_tree = use_trees();
            }
            for t in lo..hi {
                if (k..=e).contains(&t) || inner.iter().any(|&(o, c)| o <= t && t <= c) || in_tree[t] {
                    continue;
                }
                if is_id(t) && tk(t) == alias.as_slice() && is_sym(t + 1, b':') && is_sym(t + 2, b':') && !(t >= 2 && is_sym(t - 1, b':') && is_sym(t - 2, b':')) && !(t >= 1 && is_sym(t - 1, b'.')) {
                    reps.push((t, text.clone(), hi - lo));
                }
            }
        }
        k = e + 1;
    }
    if reps.is_empty() {
        return None;
    }
    // (the innermost `use` of a name wins: `use crate::b::m;` in a function over the module's `use crate::a::m;`)
    reps.sort_by_key(|r| (r.0, r.2));
    reps.dedup_by_key(|r| r.0);
    let mut out = Vec::with_capacity(s.len() + reps.len() * 16);
    let mut at = 0;
    for (t, p, _) in reps {
        out.extend_from_slice(&s[at..toks[t].1]);
        out.extend_from_slice(&p);
        at = toks[t].2;
    }
    out.extend_from_slice(&s[at..]);
    String::from_utf8(out).ok()
}

fn flatten_modules_fix_in(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>) -> Option<String> {
    let renamed = rename_module_collisions_fix(src, crates, fix);
    let src: &str = renamed.as_deref().unwrap_or(src);
    // (no module: what the renaming did, a `use std::cell::Cell as C;`)
    if !src.contains("mod") {
        return renamed;
    }
    let r = flatten_modules_inner(src);
    r.or(renamed)
}

fn flatten_modules_inner(src: &str) -> Option<String> {
    if !src.contains("mod") {
        return None;
    }
    let s = src.as_bytes();
    let toks = tokenize(s);
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    // 対応する閉じ括弧のトークン位置
    let matching = |k: usize| -> Option<usize> {
        let (open, close) = match tok(k) {
            b"{" => (b"{", b"}"),
            b"[" => (b"[", b"]"),
            b"(" => (b"(", b")"),
            _ => return None,
        };
        let mut d = 0;
        for j in k..toks.len() {
            if toks[j].0 == 1 {
                if tok(j) == open {
                    d += 1;
                } else if tok(j) == close {
                    d -= 1;
                    if d == 0 {
                        return Some(j);
                    }
                }
            }
        }
        None
    };
    let mut out = s.to_vec();
    let blank = |out: &mut Vec<u8>, a: usize, b: usize| {
        for x in &mut out[a..b] {
            if *x != b'\n' {
                *x = b' ';
            }
        }
    };
    let mut changed = false;
    let mut k = 0;
    while k + 2 < toks.len() {
        if !(toks[k].0 == 0 && tok(k) == b"mod" && toks[k + 1].0 == 0 && tok(k + 2) == b"{") {
            k += 1;
            continue;
        }
        let Some(close) = matching(k + 2) else { return None };
        // 前の `pub` / `pub(crate)` と属性 `#[..]` も含める
        let mut st = k;
        let mut is_test = false;
        loop {
            if st >= 1 && toks[st - 1].0 == 0 && tok(st - 1) == b"pub" {
                st -= 1;
                continue;
            }
            if st >= 1 && tok(st - 1) == b")" {
                // `pub(crate)`
                let mut j = st - 1;
                while j > 0 && tok(j) != b"(" {
                    j -= 1;
                }
                if j >= 1 && tok(j - 1) == b"pub" {
                    st = j - 1;
                    continue;
                }
            }
            if st >= 1 && tok(st - 1) == b"]" {
                let mut j = st - 1;
                let mut d = 0;
                loop {
                    if tok(j) == b"]" {
                        d += 1;
                    } else if tok(j) == b"[" {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                }
                if j >= 1 && tok(j - 1) == b"#" {
                    let attr = &s[toks[j].1..toks[st - 1].2];
                    let a = String::from_utf8_lossy(attr);
                    if a.contains("cfg") && a.contains("test") {
                        is_test = true;
                    }
                    st = j - 1;
                    continue;
                }
            }
            break;
        }
        changed = true;
        if is_test {
            blank(&mut out, toks[st].1, toks[close].2);
            k = close + 1;
        } else {
            blank(&mut out, toks[st].1, toks[k + 2].2);
            blank(&mut out, toks[close].1, toks[close].2);
            k += 3;
        }
    }
    changed.then(|| String::from_utf8(out).unwrap_or_default())
}

/// The macros of std and core (and the compiler's builtin ones) a program may call without defining them: a call of any
/// other name left after expansion is of a macro that does not exist
pub fn is_std_macro(n: &str) -> bool {
    matches!(
        n,
        "println" | "print" | "eprintln" | "eprint" | "format" | "format_args" | "write" | "writeln" | "panic" | "assert" | "assert_eq"
            | "assert_ne" | "debug_assert" | "debug_assert_eq" | "debug_assert_ne" | "unreachable" | "unimplemented" | "todo" | "vec"
            | "concat" | "stringify" | "env" | "option_env" | "include" | "include_str" | "include_bytes" | "file" | "line" | "column"
            | "module_path" | "cfg" | "matches" | "dbg" | "compile_error" | "thread_local" | "const_format_args" | "concat_idents"
            | "trace_macros" | "log_syntax" | "format_args_nl" | "assert_matches" | "debug_assert_matches" | "try" | "ready"
            | "pin" | "addr_of" | "addr_of_mut" | "offset_of" | "cfg_select" | "cfg_match" | "global_asm" | "asm" | "naked_asm"
            | "is_x86_feature_detected" | "select_unpredictable" | "join"
    )
}

/// Whether position p is inside the arguments of a macro invocation (`m! { .. }`, `macro_rules! m { .. }`): the group
/// that holds p opens right after a `!`, or after `! name`. Doc comments there are tokens the macro reads (roughly:
/// brackets in strings and comments before p are counted too)
fn in_macro_args(s: &[u8], p: usize) -> bool {
    let mut depth = 0i32;
    let mut k = p;
    while k > 0 {
        k -= 1;
        match s[k] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                if depth == 0 {
                    // (the token before the opener)
                    let mut j = k;
                    while j > 0 && s[j - 1].is_ascii_whitespace() {
                        j -= 1;
                    }
                    if j > 0 && s[j - 1] == b'!' {
                        return true;
                    }
                    // (`macro_rules! name {`)
                    let mut i = j;
                    while i > 0 && (s[i - 1].is_ascii_alphanumeric() || s[i - 1] == b'_') {
                        i -= 1;
                    }
                    let mut h = i;
                    while h > 0 && s[h - 1].is_ascii_whitespace() {
                        h -= 1;
                    }
                    return i < j && h > 0 && s[h - 1] == b'!';
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    false
}

/// An outer doc comment the source ends in, with only whitespace and plain comments after it: rustc's "expected item
/// after doc comment", at the last doc comment (`///`, not `////`; or a `/** .. */`). Read from the source as written,
/// after its last token: the expansion takes `macro_rules!` definitions out of the text, which leaves their doc comments last
pub fn doc_at_end(src: &str) -> Option<String> {
    // (only a source whose last line holds a comment: the rest end in a token)
    let t = src.trim_end();
    let last = &t[t.rfind('\n').map_or(0, |k| k + 1)..];
    if !last.contains("//") && !t.ends_with("*/") {
        return None;
    }
    // (after the last token there are only whitespace and comments: read them as comments, whole, so a `/**` inside
    // a line comment or a block comment is not an opener; a plain comment after the doc is whitespace)
    let s = src.as_bytes();
    let n = s.len();
    let mut i = tokenize(s).last().map_or(0, |t| t.2);
    let mut doc = None;
    while i < n {
        if s[i].is_ascii_whitespace() {
            i += 1;
        } else if s[i..].starts_with(b"//") {
            let e = s[i..].iter().position(|&c| c == b'\n').map_or(n, |k| i + k);
            if s[i..].starts_with(b"///") && !s[i..].starts_with(b"////") {
                doc = Some(i);
            }
            i = e;
        } else if s[i..].starts_with(b"/*") {
            let (b, mut d) = (i, 0u32);
            while i < n {
                if s[i..].starts_with(b"/*") {
                    d += 1;
                    i += 2;
                } else if s[i..].starts_with(b"*/") {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            let c = &s[b..i];
            if c.starts_with(b"/**") && !c.starts_with(b"/***") && c != b"/**/" {
                doc = Some(b);
            }
        } else {
            return None;
        }
    }
    doc.map(|at| {
        let l = 1 + src[..at].matches('\n').count();
        let c = at - src[..at].rfind('\n').map_or(0, |k| k + 1) + 1;
        format!("{l}:{c}: expected item after doc comment")
    })
}

/// E0585: an outer doc comment right before `}` / `;` / `,` (outside a macro's arguments) documents nothing
#[inline(never)]
fn dangling_docs(s: &[u8], docs: Vec<(usize, usize)>) -> Result<(), String> {
    let n = s.len();
    // (E0585: an outer doc comment right before `}` / `;` / `,` documents nothing. One forward pass: a doc that starts
    // before the previous lookahead's stop ends at the same token)
    let mut stop = 0usize;
    for (d0, d1) in docs {
        if d0 < stop {
            continue;
        }
        let mut j = d1;
        loop {
            if j < n && s[j].is_ascii_whitespace() {
                j += 1;
            } else if j + 1 < n && s[j] == b'/' && s[j + 1] == b'/' {
                while j < n && s[j] != b'\n' {
                    j += 1;
                }
            } else if j + 1 < n && s[j] == b'/' && s[j + 1] == b'*' {
                // (a block comment, nested ones in it)
                let mut d = 0;
                while j < n {
                    if j + 1 < n && s[j] == b'/' && s[j + 1] == b'*' {
                        d += 1;
                        j += 2;
                    } else if j + 1 < n && s[j] == b'*' && s[j + 1] == b'/' {
                        d -= 1;
                        j += 2;
                        if d == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
            } else {
                break;
            }
        }
        stop = j;
        if j < n && matches!(s[j], b'}' | b';' | b',') && !in_macro_args(s, d0) {
            let line = s[..d0].iter().filter(|&&c| c == b'\n').count() + 1;
            let col = d0 - s[..d0].iter().rposition(|&c| c == b'\n').map_or(0, |k| k + 1) + 1;
            return Err(format!("{line}:{col}: found a documentation comment that doesn't document anything (E0585)"));
        }
    }
    Ok(())
}

/// Whether n is a primitive type's name (`u8`, `char`, `str`, ..)
fn is_primitive_name(n: &[u8]) -> bool {
    matches!(
        n,
        b"bool" | b"char" | b"str" | b"u8" | b"u16" | b"u32" | b"u64" | b"u128" | b"usize" | b"i8" | b"i16" | b"i32" | b"i64" | b"i128" | b"isize" | b"f16" | b"f32" | b"f64" | b"f128"
    )
}

/// Whether the name token that starts at byte `at` of s is a number's suffix, `0u8` / `1_000u32` / `0x800u32` / `1.5f32`:
/// tokenize reads the digits apart and the rest as a name, and no name starts right after a digit
fn number_suffix(s: &[u8], at: usize) -> bool {
    at >= 1 && s[at - 1].is_ascii_digit()
}

/// Namespaces a name is in (rename_module_collisions): types (struct, enum, trait, type, union) and values (fn, const,
/// static; a struct's constructor). A type's place looks only at types, so `*mut stat` beside `fn stat` is the struct
const NS_TYPE: u8 = 1;
const NS_VALUE: u8 = 2;
const NS_ANY: u8 = 3;
/// Macros, which no item of a module is: only what a `use` imports from std under another name (`pub use std::println
/// as say;`) is one, and a macro's name (`m::say!(..)`) looks only at those
const NS_MACRO: u8 = 4;

/// What a name is through a module's globs: an item renamed (its new name, its namespaces), an item that keeps its name
/// (the module that defines it), nothing, or more than one (ambiguous, E0659: left as written, rustc reports it)
#[derive(Clone, PartialEq)]
enum GlobHit {
    None,
    Item(Vec<u8>, u8),
    Own(usize),
    Ambiguous,
}

/// A module a path segment names through globs: none, one, or more than one
#[derive(Clone, Copy, PartialEq)]
enum ModHit {
    None,
    One(usize),
    Ambiguous,
}

/// One more candidate: whether the candidates stay one item (equal ones are one)
fn add_hit(found: &mut GlobHit, r: GlobHit) -> bool {
    match (&*found, r) {
        (_, GlobHit::None) => true,
        (_, GlobHit::Ambiguous) => false,
        (GlobHit::None, r) => {
            *found = r;
            true
        }
        (f, r) => *f == r,
    }
}

/// The key of an import of the name's item of namespace ns only (`sigaction#t`): no identifier has a `#`
fn ns_key(n: &[u8], ns: u8) -> Vec<u8> {
    let mut k = n.to_vec();
    k.extend_from_slice(if ns == NS_TYPE { b"#t" } else { b"#v" });
    k
}

/// The item of the other namespace than ns that module m's name n is, when it is another than nn (one namespace's
/// item found first): libc's `fn sigaction` and the `struct sigaction` a glob of the same module brings
fn other_ns_item(names: &Names, m: usize, n: &[u8], ns: u8, nn: &[u8]) -> Option<(Vec<u8>, u8)> {
    if ns != NS_TYPE && ns != NS_VALUE {
        return None;
    }
    let other = NS_ANY ^ ns;
    // (only through a glob of m's, or m's own import of the other namespace's item under `name#t` / `name#v`: a
    // re-export by name, `pub use crate::api::Entry;`, of a fn and a glob's struct. m's own items and its other
    // imports of the name are one namespace's)
    if !names.adj.contains_key(&m) && !(names.ns_imports && names.imports.contains_key(&(m, ns_key(n, other)))) {
        return None;
    }
    match names.item_at(m, m, n, other) {
        GlobHit::Item(n2, ns2) if n2 != nn && ns2 & other != 0 => Some((n2, other)),
        _ => None,
    }
}

/// Name resolution through glob re-exports, globs of globs too, as the module `viewer` sees them: a glob brings the
/// names visible from where it is written (`pub` ones, and all of an ancestor's: `use super::*;`), so a private
/// `mod linux` in libc's unix/linux_like does not collide with new's `pub(crate) mod linux`. Each (viewer, module,
/// name, namespaces) and (viewer, module, segment) is worked out once (a graph whose routes meet is not walked once
/// per route); one in progress is nothing, which stops a cycle (`pub use super::dsl::*;` back and forth)
struct Names<'a> {
    mods: &'a [(Vec<Vec<u8>>, usize, usize)],
    /// whether each module is declared `pub` (in any form)
    mod_pub: &'a [bool],
    mod_of_path: &'a LMap<Vec<Vec<u8>>, usize>,
    mod_alias: &'a LMap<(usize, Vec<u8>), usize>,
    #[allow(dead_code)]
    globs: &'a [(usize, usize)],
    /// the globs by the module they are written in
    adj: LMap<usize, Vec<(usize, usize)>>,
    /// how many lookups met one in progress (a cycle): a result found then is not kept, it may lack what the
    /// unfinished lookup gives
    cycles: std::cell::Cell<usize>,
    /// the globs, aliases, items and imports written `pub`
    vis: &'a Vis,
    renamed: &'a LMap<(usize, Vec<u8>), Vec<u8>>,
    def_ns: &'a LMap<(usize, Vec<u8>), u8>,
    /// the names imported by `use` (not globs): (scope, name) -> (new name, namespaces)
    imports: &'a LMap<(usize, Vec<u8>), (Vec<u8>, u8)>,
    ns_imports: bool,
    items: std::cell::RefCell<LMap<(usize, usize, Vec<u8>, u8), GlobHit>>,
    modules: std::cell::RefCell<LMap<(usize, usize, Vec<u8>), ModHit>>,
    /// the lookups in progress
    active_items: std::cell::RefCell<LSet<(usize, usize, Vec<u8>, u8)>>,
    active_mods: std::cell::RefCell<LSet<(usize, usize, Vec<u8>)>>,
}

/// What is written `pub` (rename_module_collisions), for Names
#[derive(Default)]
struct Vis {
    globs: LSet<(usize, usize)>,
    aliases: LSet<(usize, Vec<u8>)>,
    defs: LSet<(usize, Vec<u8>)>,
    imports: LSet<(usize, Vec<u8>)>,
}

impl<'a> Names<'a> {
    /// Whether module viewer is t or inside it (sees t's private items)
    fn within(&self, viewer: usize, t: usize) -> bool {
        t == usize::MAX || (viewer != usize::MAX && self.mods[viewer].0.starts_with(&self.mods[t].0))
    }

    /// m's child module seg, or one m imports by name (glibc's `bits` has `pub(crate) use super::sysdeps::..::{types};`),
    /// that viewer sees
    fn child(&self, viewer: usize, m: usize, seg: &[u8]) -> Option<usize> {
        let mut p: Vec<Vec<u8>> = if m == usize::MAX { vec![] } else { self.mods[m].0.clone() };
        p.push(seg.to_vec());
        let inner = self.within(viewer, m);
        if let Some(&c) = self.mod_of_path.get(&p) {
            if inner || self.mod_pub[c] {
                return Some(c);
            }
        }
        let key = (m, seg.to_vec());
        self.mod_alias.get(&key).copied().filter(|_| inner || self.vis.aliases.contains(&key))
    }

    /// m's globs that viewer sees
    fn globs_of(&self, viewer: usize, m: usize) -> impl Iterator<Item = usize> + '_ {
        let inner = self.within(viewer, m);
        self.adj.get(&m).into_iter().flatten().filter(move |g| inner || self.vis.globs.contains(g)).map(|g| g.1)
    }

    /// The module seg that m's globs bring in (libc: `pub use new::*;` at the root, `pub(crate) use linux_uapi::*;` in
    /// new, so `crate::linux`)
    fn module_via_globs(&self, viewer: usize, m: usize, seg: &[u8]) -> ModHit {
        let key = (viewer, m, seg.to_vec());
        if let Some(&r) = self.modules.borrow().get(&key) {
            if r == ModHit::None && self.active_mods.borrow().contains(&key) {
                self.cycles.set(self.cycles.get() + 1);
            }
            return r;
        }
        let c0 = self.cycles.get();
        self.active_mods.borrow_mut().insert(key.clone());
        self.modules.borrow_mut().insert(key.clone(), ModHit::None);
        let mut found = ModHit::None;
        let targets: Vec<usize> = self.globs_of(viewer, m).collect();
        for t in targets {
            let r = match self.child(viewer, t, seg) {
                Some(c) => ModHit::One(c),
                None => self.module_via_globs(viewer, t, seg),
            };
            found = match (found, r) {
                (f, ModHit::None) => f,
                (_, ModHit::Ambiguous) => ModHit::Ambiguous,
                (ModHit::None, r) => r,
                (f, r) if f == r => f,
                _ => ModHit::Ambiguous,
            };
            if found == ModHit::Ambiguous {
                break;
            }
        }
        self.active_mods.borrow_mut().remove(&key);
        if self.cycles.get() == c0 {
            self.modules.borrow_mut().insert(key, found);
        } else {
            self.modules.borrow_mut().remove(&key);
        }
        found
    }

    /// From module m along segs, as viewer sees them: each a child module (first) or one the globs bring in, if it is
    /// one
    fn walk(&self, _viewer: usize, m: usize, segs: &[Vec<u8>]) -> Option<usize> {
        // (each step as the module it is in sees it: a path that names a module's own child reaches it (whether it
        //  may is another check, E0603), and what its globs bring is what that module imports)
        let mut cur = m;
        for seg in segs {
            cur = match self.child(cur, cur, seg) {
                Some(c) => c,
                None => match self.module_via_globs(cur, cur, seg) {
                    ModHit::One(c) => c,
                    _ => return None,
                },
            };
        }
        Some(cur)
    }

    /// The name n in module t as viewer sees it, in the namespaces want: t's own item (renamed, or keeping its name),
    /// what t imports by name, else what t's globs bring
    fn item_at(&self, viewer: usize, t: usize, n: &[u8], want: u8) -> GlobHit {
        let key = (t, n.to_vec());
        let inner = self.within(viewer, t);
        if let Some(&ns) = self.def_ns.get(&key) {
            if ns & want != 0 && (inner || self.vis.defs.contains(&key)) {
                return match self.renamed.get(&key) {
                    Some(nn) => GlobHit::Item(nn.clone(), ns),
                    None => GlobHit::Own(t),
                };
            }
        }
        // (an import of one namespace's item, beside another's of the name: other_ns_item)
        if self.ns_imports && (want == NS_TYPE || want == NS_VALUE) {
            if let Some((nn, ns)) = self.imports.get(&(t, ns_key(n, want))) {
                if inner || self.vis.imports.contains(&key) {
                    return GlobHit::Item(nn.clone(), *ns);
                }
            }
        }
        if let Some((nn, ns)) = self.imports.get(&key) {
            if ns & want != 0 && (inner || self.vis.imports.contains(&key)) {
                return GlobHit::Item(nn.clone(), *ns);
            }
        }
        self.via_globs(viewer, t, n, want)
    }

    /// The name n through m's globs, each followed to its end: one item, or ambiguous
    fn via_globs(&self, viewer: usize, m: usize, n: &[u8], want: u8) -> GlobHit {
        let key = (viewer, m, n.to_vec(), want);
        if let Some(r) = self.items.borrow().get(&key) {
            if *r == GlobHit::None && self.active_items.borrow().contains(&key) {
                self.cycles.set(self.cycles.get() + 1);
            }
            return r.clone();
        }
        let c0 = self.cycles.get();
        self.active_items.borrow_mut().insert(key.clone());
        self.items.borrow_mut().insert(key.clone(), GlobHit::None);
        let mut found = GlobHit::None;
        let targets: Vec<usize> = self.globs_of(viewer, m).collect();
        for t in targets {
            let r = self.item_at(viewer, t, n, want);
            if !add_hit(&mut found, r) {
                found = GlobHit::Ambiguous;
                break;
            }
        }
        self.active_items.borrow_mut().remove(&key);
        if self.cycles.get() == c0 {
            self.items.borrow_mut().insert(key, found.clone());
        } else {
            self.items.borrow_mut().remove(&key);
        }
        found
    }

    /// The new name of n as module m sees it (want: the namespaces of the place), or None to keep it as written. A
    /// type's place that finds no type takes any item (an item zustc does not classify)
    fn lookup(&self, m: usize, n: &[u8], want: u8) -> Option<Vec<u8>> {
        self.lookup_from(m, m, n, want)
    }

    /// The new name of n as module m sees it, a type only (a primitive's name: a value of that name does not shadow it)
    fn lookup_type(&self, m: usize, n: &[u8]) -> Option<Vec<u8>> {
        match self.item_at(m, m, n, NS_TYPE) {
            GlobHit::Item(nn, _) => Some(nn),
            _ => None,
        }
    }

    /// lookup, of module m's n as viewer (where the path is written) sees it
    fn lookup_from(&self, viewer: usize, m: usize, n: &[u8], want: u8) -> Option<Vec<u8>> {
        // (m's item as m sees it: a path to a module's private item still names it (the test harness calls
        //  `b::foo()` for a private `#[test] fn foo`), and m's globs bring what m imports)
        let _ = viewer;
        match self.item_at(m, m, n, want) {
            GlobHit::Item(nn, _) => Some(nn),
            // (a macro's name takes nothing else)
            GlobHit::None if want != NS_ANY && want != NS_MACRO => self.lookup_from(m, m, n, NS_ANY),
            _ => None,
        }
    }
}

/// The tokens (lexer tokens, s their text) that name no item, which renaming an item leaves as they are: an
/// attribute's, `#[derive(Eq, Debug)]` / `#[repr(C)]` / `#![allow(x)]` (its path and a derive's names are macros, the
/// rest its own arguments), and a macro call's name, `write!(..)` / the `mac` of `m::mac![..]` (the macro namespace,
/// where no item is). typenum's `#[derive(Eq, ..)]` took the `pub type Eq<A, B>` that `operator_aliases::*` brings,
/// `typenum__operator_aliases__Eq`: no derive has that name. The modules before a macro's name are a path's, renamed
/// with their module: two fn bodies' `mod m` hoisted as `m__h0` / `m__h1` have `m__h0::say!(..)` / `m__h1::say!(..)`
fn macro_ns_toks(toks: &[(u8, usize, usize)], s: &[u8]) -> Vec<bool> {
    let sym = |k: usize, c: u8| toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let mut out = vec![false; toks.len()];
    let mut k = 0;
    while k + 2 < toks.len() {
        if sym(k, b'#') && (sym(k + 1, b'[') || (sym(k + 1, b'!') && sym(k + 2, b'['))) {
            let open = if sym(k + 1, b'[') { k + 1 } else { k + 2 };
            let mut d = 0usize;
            let mut e = open;
            while e < toks.len() {
                if sym(e, b'[') {
                    d += 1;
                } else if sym(e, b']') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                e += 1;
            }
            out[open..e.min(toks.len())].fill(true);
            k = e + 1;
            continue;
        }
        if toks[k].0 == 0 && sym(k + 1, b'!') && (sym(k + 2, b'(') || sym(k + 2, b'[') || sym(k + 2, b'{')) {
            out[k] = true;
        }
        k += 1;
    }
    out
}

/// Whether the item, `mod` or `use` whose keyword is at token kw is written `pub` (`pub`, `pub(crate)`, ..; past
/// `unsafe` / `async` / `const` / `extern "C"` / `default` / `safe` before a fn)
fn pub_before(toks: &[(u8, usize, usize)], s: &[u8], kw: usize) -> bool {
    let sym = |k: usize, c: u8| toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let id = |k: usize, w: &[u8]| toks[k].0 == 0 && &s[toks[k].1..toks[k].2] == w;
    let mut q = kw;
    while q >= 1 && (toks[q - 1].0 == 2 || ["unsafe", "async", "const", "extern", "default", "safe"].iter().any(|w| id(q - 1, w.as_bytes()))) {
        q -= 1;
    }
    if q >= 1 && id(q - 1, b"pub") {
        return true;
    }
    if q >= 1 && sym(q - 1, b')') {
        let mut j = q - 1;
        while j > 0 && !sym(j, b'(') {
            j -= 1;
        }
        // (`pub(self)` is private: srcmap writes a private `mod x;`'s file as `pub(self) mod x { .. }`)
        let only_self = j + 2 == q - 1 && id(j + 1, b"self");
        return j >= 1 && id(j - 1, b"pub") && !only_self;
    }
    false
}

/// Whether the name at token q is in a type's place by what comes before it (lexer tokens, s their text): after
/// `*mut` / `*const`, `->`, a single `:`, a generic `<`, `as`, or a reference type `&`, `&mut`, `&'a`, `&'a mut`
/// after one of those (a `&` elsewhere borrows a value: `&pcg32(state)`)
fn type_place_before(toks: &[(u8, usize, usize)], s: &[u8], q: usize) -> bool {
    let sym = |k: usize, c: u8| toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let id = |k: usize, w: &[u8]| toks[k].0 == 0 && &s[toks[k].1..toks[k].2] == w;
    // (a lifetime is its name's token, the `'` before it skipped)
    let lt = |k: usize| toks[k].0 == 0 && toks[k].1 >= 1 && s[toks[k].1 - 1] == b'\'';
    let after = |p: usize| {
        (sym(p, b'>') && p >= 1 && sym(p - 1, b'-'))
            || (sym(p, b':') && !(p >= 1 && sym(p - 1, b':')) && !(p + 1 < toks.len() && sym(p + 1, b':')))
            // (`<` opening type arguments: after a type's name (capitalized), `::` or `impl` / `dyn`; not `i < MAX`)
            || (sym(p, b'<')
                && p >= 1
                && (sym(p - 1, b':')
                    || id(p - 1, b"impl")
                    || id(p - 1, b"dyn")
                    || (toks[p - 1].0 == 0 && s[toks[p - 1].1].is_ascii_uppercase())))
    };
    if q == 0 {
        return false;
    }
    let mut p = q - 1;
    if (id(p, b"mut") || id(p, b"const")) && p >= 1 && sym(p - 1, b'*') {
        return true;
    }
    if id(p, b"as") || after(p) {
        return true;
    }
    // (the right-hand side of a type alias, `type Alias<T> = T;`: back over its name and generics to `type`, within
    //  the item)
    if sym(p, b'=') {
        let mut j = p;
        let mut n = 0;
        while j > 0 && n < 64 {
            j -= 1;
            n += 1;
            if id(j, b"type") {
                return true;
            }
            if sym(j, b';') || sym(j, b'{') || sym(j, b'}') || sym(j, b'=') || sym(j, b'(') {
                return false;
            }
        }
    }
    // (`&T`, `&mut T`, `&'a T`, `&'a mut T`)
    if id(p, b"mut") && p >= 1 {
        p -= 1;
    }
    if lt(p) && p >= 1 {
        p -= 1;
    }
    sym(p, b'&') && p >= 1 && after(p - 1)
}
