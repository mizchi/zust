//! The built-in prelude: in place of core, common types written in the subset this compiler handles.
//! It is appended to the user's source and compiled with it (only what is used is code-generated).

/// The allocator `Box::new` calls (libc's malloc)
pub const BOX_ALLOC: &str = "__exr_malloc";
/// The deallocation function Box's drop calls (libc's free)
pub const BOX_FREE: &str = "__exr_free";
/// drop glue: codegen builds the body of `__exr_drop_in_place::<T>` for each type
pub const DROP_GLUE: &str = "__exr_drop_in_place";

/// The prelude's text (in place of core / alloc, written in Rust). build.rs splits it into items ahead of time
pub const PRELUDE: &str = include_str!("prelude_lib.rs.in");

include!(concat!(env!("OUT_DIR"), "/prelude_items.rs"));

type HashMap<K, V> = crate::fxhash::FxMap<K, V>;
use std::sync::OnceLock;

use crate::lexer::{ItemKind, SrcItem};

/// The prelude's items (lexed only once): (item, the name it defines, whether it is usable only through core::)
fn items() -> &'static [(SrcItem, Option<String>, bool)] {
    static ITEMS: OnceLock<Vec<(SrcItem, Option<String>, bool)>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        // (build.rs has already done the splitting)
        PRELUDE_ITEMS
            .iter()
            .map(|&(start, end, line, k, a, idents)| SrcItem {
                start: start as usize,
                end: end as usize,
                line: line as usize,
                hash: 0,
                idents: idents.to_vec(),
                kind: match k {
                    0 => ItemKind::Normal,
                    1 => ItemKind::ImplHeader,
                    2 => ItemKind::ImplMember(a),
                    3 => ItemKind::TraitHeader,
                    4 => ItemKind::TraitMember(a),
                    _ => ItemKind::InnerAttr,
                },
            })
            .map(|it| {
                let mut name = defined_name(PRELUDE, &it);
                // (a member of an impl of a radix or exponent fmt trait (`impl LowerHex for u8`): looked up by the
                // trait's name, which a program that uses the impls names, not by `fmt`, which every program that
                // formats needs: the first lookup of each round then passes over them, not their gate)
                if let ItemKind::ImplMember(h) = it.kind {
                    let mut hw = PRELUDE_ITEMS[h as usize].5.iter().map(|&(a, b)| &PRELUDE[a as usize..b as usize]);
                    if let (Some("impl"), Some(t), Some("for")) = (hw.next(), hw.next(), hw.next()) {
                        if matches!(t, "Binary" | "Octal" | "LowerHex" | "UpperHex" | "LowerExp" | "UpperExp") {
                            name = Some(t.to_string());
                        }
                    }
                }
                let core_only = it
                    .idents
                    .iter()
                    .any(|&(a, b)| PRELUDE[a as usize..b as usize].starts_with("__core_"));
                (it, name, core_only)
            })
            .collect()
    })
}

/// Whether a call that makes an iterator (`.iter()` / `.into_iter()` / `(a..b)`) is followed by a method
/// other than `rev` / `enumerate` / `copied` / `cloned` (which `for` handles directly), as in `v.iter().map(..)`
fn iter_chain(src: &str) -> bool {
    let b = src.as_bytes();
    let starts = ["iter()", "iter_mut()", "into_iter()"];
    let mut found = vec![];
    for s in starts {
        let mut i = 0;
        while let Some(k) = src[i..].find(s) {
            found.push(i + k + s.len());
            i += k + s.len();
        }
    }
    // After the `)` of `(a..b).`
    let mut i = 0;
    while let Some(k) = src[i..].find("..") {
        let mut j = i + k + 2;
        // (`(0..v.len())`: calls inside the range's end are skipped over)
        let mut depth = 0u32;
        while j < b.len() && !matches!(b[j], b';' | b'{' | b'}' | b'\n') {
            match b[j] {
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        if j < b.len() && b[j] == b')' {
            found.push(j + 1);
        }
        i += k + 2;
    }
    for mut j in found {
        // Walk the chain of `.name(..)`
        loop {
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= b.len() || b[j] != b'.' {
                break;
            }
            let st = j + 1;
            let mut e = st;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            let name = &src[st..e];
            if !matches!(name, "rev" | "enumerate" | "copied" | "cloned") {
                return !name.is_empty();
            }
            // Skip the argument parentheses
            j = e;
            if j < b.len() && b[j] == b'(' {
                let mut d = 0;
                while j < b.len() {
                    match b[j] {
                        b'(' => d += 1,
                        b')' => {
                            d -= 1;
                            if d == 0 {
                                j += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
            }
        }
    }
    false
}

/// Whether the identifier src[a..b] is at a method call (`.name(` / `.name::<`), not a field (`.next`)
fn method_call_at(src: &[u8], a: usize, b: usize) -> bool {
    if a > 0 && src[a - 1] == b'.' && b < src.len() && (src[b] == b'(' || src[b] == b':') {
        return true;
    }
    // (macro expansions put spaces between the tokens: `p1 . zip (p2)`)
    let mut i = a;
    while i > 0 && src[i - 1] == b' ' {
        i -= 1;
    }
    let mut j = b;
    while j < src.len() && src[j] == b' ' {
        j += 1;
    }
    (i < a || j > b) && i > 0 && src[i - 1] == b'.' && (i < 2 || src[i - 2] != b'.') && j < src.len() && (src[j] == b'(' || src[j] == b':')
}

/// Whether h is the header of the Iterator trait or an Iterator impl (or IntoIter's Drop)
fn is_iterator_trait(h: usize) -> bool {
    // (computed once per item: the selection asks for each impl member on every build)
    static T: OnceLock<Vec<bool>> = OnceLock::new();
    T.get_or_init(|| {
        items()
            .iter()
            // (asked for headers only: other items are not scanned)
            .map(|(it, _, _)| {
                matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader)
                    && it.idents.iter().any(|&(a, b)| matches!(&PRELUDE[a as usize..b as usize], "Iterator" | "IntoIter"))
            })
            .collect()
    })[h]
}

/// The `__exr_str_<m>` / `__exr_char_<m>` helpers: method name m → helper names
fn helper_suffixes() -> &'static HashMap<&'static str, Vec<&'static str>> {
    static H: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    H.get_or_init(|| {
        let mut m: HashMap<&'static str, Vec<&'static str>> = HashMap::default();
        for (_, name, _) in items() {
            let Some(n) = name.as_deref() else { continue };
            for pre in ["__exr_str_", "__exr_char_"] {
                if let Some(suf) = n.strip_prefix(pre) {
                    if !suf.is_empty() {
                        // (`trim_matches_fn` is the version taking a closure pattern: looked up by the method name `trim_matches`)
                        let suf = suf.strip_suffix("_fn").unwrap_or(suf);
                        m.entry(suf).or_default().push(n);
                    }
                }
            }
        }
        m
    })
}

/// Whether h declares a comparison trait (PartialEq / Eq / PartialOrd / Ord)
/// Whether the selection's `wanted` can be true for item i by more than its name (others only when named)
fn by_more_than_name(i: usize) -> bool {
    static T: OnceLock<Vec<bool>> = OnceLock::new();
    T.get_or_init(|| {
        items()
            .iter()
            .map(|(it, name, _)| {
                let special = matches!(
                    name.as_deref(),
                    Some("Option" | "Result" | "__exr_str_cmp" | "Range" | "RangeInclusive" | "__err_mark" | "eq")
                ) || name.as_deref().is_some_and(|n| n == BOX_ALLOC || n == BOX_FREE || n == DROP_GLUE || n.starts_with("__exr_fnptr"));
                special
                    || match it.kind {
                        ItemKind::ImplMember(h) => is_seq_eq_impl(h as usize) || (owner(h as usize).is_some() && is_drop_impl(h as usize)),
                        _ => false,
                    }
            })
            .collect()
    })[i]
}

/// Whether h is an `impl Drop for ..` header (computed once per item)
fn is_drop_impl(h: usize) -> bool {
    static T: OnceLock<Vec<bool>> = OnceLock::new();
    T.get_or_init(|| {
        items()
            .iter()
            .map(|(it, _, _)| matches!(it.kind, ItemKind::ImplHeader) && it.idents.iter().any(|&(a, b)| &PRELUDE[a as usize..b as usize] == "Drop"))
            .collect()
    })[h]
}

fn is_cmp_trait(h: usize) -> bool {
    // (computed once per item: asked for every trait member on each round of the selection)
    static T: OnceLock<Vec<bool>> = OnceLock::new();
    T.get_or_init(|| {
        items()
            .iter()
            .map(|(it, _, _)| {
                matches!(it.kind, ItemKind::TraitHeader)
                    && it.idents.iter().any(|&(a, b)| {
                        matches!(&PRELUDE[a as usize..b as usize], "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Clone" | "Copy" | "Default")
                    })
            })
            .collect()
    })[h]
}

/// The operator traits (their declarations are added only when the user impls one by hand)
const OP_TRAITS: &[&str] = &[
    "Add", "Sub", "Mul", "Div", "Rem", "Neg", "Not", "BitAnd", "BitOr", "BitXor", "AddAssign", "SubAssign",
    "MulAssign", "DivAssign", "RemAssign", "BitAndAssign", "BitOrAssign", "BitXorAssign", "ShlAssign", "ShrAssign",
    "Shl", "Shr", "Index", "IndexMut", "From", "Deref", "IntoIterator", "Error", "AsRef", "AsMut", "Into", "Borrow", "BorrowMut",
    "FromIterator", "Extend",
];

/// The trait's name, when h declares an operator trait (computed once per item)
fn op_trait_name(h: usize) -> Option<&'static str> {
    static T: OnceLock<Vec<Option<&'static str>>> = OnceLock::new();
    T.get_or_init(|| {
        items()
            .iter()
            .map(|(it, _, _)| {
                if !matches!(it.kind, ItemKind::TraitHeader) {
                    return None;
                }
                it.idents.iter().map(|&(a, b)| &PRELUDE[a as usize..b as usize]).find(|w| OP_TRAITS.contains(w))
            })
            .collect()
    })[h]
}

/// Trait declarations added only when the name is needed (members with common names like `write` / `hash`)
fn gated_trait(h: usize) -> Option<&'static str> {
    // (computed once per item)
    static T: OnceLock<Vec<Option<&'static str>>> = OnceLock::new();
    T.get_or_init(|| {
        items()
            .iter()
            .map(|(it, _, _)| {
                if !matches!(it.kind, ItemKind::TraitHeader) {
                    return None;
                }
                it.idents.iter().map(|&(a, b)| &PRELUDE[a as usize..b as usize]).find(|w| {
                    matches!(*w, "Hasher" | "Hash" | "__BoolThen" | "__AsBytes" | "__PathExt" | "__AsPathStr" | "BufRead" | "Read" | "Seek" | "Write" | "__FWr" | "ExactSizeIterator" | "BuildHasher"
                        // (std::os::unix's: `size()` / `mode()` / `as_raw_fd()` of a program's own are no reason for them)
                        | "MetadataExt" | "OpenOptionsExt" | "PermissionsExt" | "AsRawFd" | "FromRawFd" | "IntoRawFd" | "JoinHandleExt")
                        || matches!(*w, "Binary" | "Octal" | "LowerHex" | "UpperHex" | "LowerExp" | "UpperExp" | "Pointer")
                })
            })
            .collect()
    })[h]
}

/// A trait impl added only when the trait's name is needed (`impl Read for &[u8]`)
fn gated_impl(h: usize) -> Option<&'static str> {
    // (computed once per item, as is_iterator_trait)
    static T: OnceLock<Vec<Option<&'static str>>> = OnceLock::new();
    T.get_or_init(|| {
        items()
            .iter()
            .map(|(it, _, _)| {
                if !matches!(it.kind, ItemKind::ImplHeader) {
                    return None;
                }
                let ws: Vec<&'static str> = it.idents.iter().map(|&(a, b)| &PRELUDE[a as usize..b as usize]).collect();
                match ws.as_slice() {
                    ["impl", t, "for", ..] if matches!(*t, "Read" | "BufRead" | "Seek" | "Write" | "__FWr") => Some(*t),
                    // (the numbers' impls of the radix and exponent fmt traits, only with the trait)
                    ["impl", t, "for", ..] if matches!(*t, "Binary" | "Octal" | "LowerHex" | "UpperHex" | "LowerExp" | "UpperExp") => Some(*t),
                    // (`{:?}` of a range: only for a program that formats with Debug)
                    ["impl", "T", "Debug", "for", r, ..] if matches!(*r, "Range" | "RangeInclusive") => Some("__range_debug"),
                    _ => None,
                }
            })
            .collect()
    })[h]
}

/// Names (last segment) of the traits the user impls by hand (`impl Ord for S` / `impl<T> Add for W<T>`)
fn user_impl_traits(src: &str) -> Vec<&str> {
    let mut out = vec![];
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find("impl") {
        let mut j = i + k + 4;
        i = j;
        let skip_ws = |j: &mut usize| {
            while *j < b.len() && b[*j].is_ascii_whitespace() {
                *j += 1;
            }
        };
        skip_ws(&mut j);
        if j < b.len() && b[j] == b'<' {
            let mut depth = 0;
            while j < b.len() {
                match b[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            skip_ws(&mut j);
        }
        let start = j;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b':') {
            j += 1;
        }
        let name = src[start..j].rsplit("::").next().unwrap_or("");
        // (skip the type arguments of `impl Mul<i32> for V`; a macro's expansion spaces them out, `Mul  < i32 > for V`)
        skip_ws(&mut j);
        if j < b.len() && b[j] == b'<' {
            let mut depth = 0;
            while j < b.len() {
                match b[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        skip_ws(&mut j);
        if !name.is_empty() && src[j..].starts_with("for") {
            out.push(name);
        }
    }
    out
}

/// Whether the user implements a comparison trait by hand (`impl Ord for S` / `impl<T> PartialEq for W<T>`)
#[allow(dead_code)]
fn impls_cmp_trait(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find("impl") {
        let mut j = i + k + 4;
        i = j;
        let skip_ws = |j: &mut usize| {
            while *j < b.len() && b[*j].is_ascii_whitespace() {
                *j += 1;
            }
        };
        skip_ws(&mut j);
        if j < b.len() && b[j] == b'<' {
            let mut depth = 0;
            while j < b.len() {
                match b[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            skip_ws(&mut j);
        }
        let start = j;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b':') {
            j += 1;
        }
        let name = src[start..j].rsplit("::").next().unwrap_or("");
        if matches!(name, "PartialEq" | "Eq" | "PartialOrd" | "Ord") {
            skip_ws(&mut j);
            if src[j..].starts_with("for") {
                return true;
            }
        }
    }
    false
}

/// Whether the source seems to use an unbounded range such as `(1..)` as a value (whether `..)` appears)
fn open_range(src: &str) -> bool {
    // `(1..)` / `(start..)`: only an identifier or digits between `(` and `..)`
    let b = src.as_bytes();
    src.match_indices("..)").any(|(i, _)| {
        let mut k = i;
        while k > 0 && (b[k - 1].is_ascii_alphanumeric() || b[k - 1] == b'_') {
            k -= 1;
        }
        k < i && k > 0 && b[k - 1] == b'('
    })
}

/// Whether a format string has an exponent spec such as `{:e}` / `{:.2E}`
fn fmt_uses_exp(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find('{') {
        let st = i + k + 1;
        let mut j = st;
        while j < b.len() && b[j] != b'}' && b[j] != b'{' && b[j] != b'\n' && j - st < 24 {
            j += 1;
        }
        if j < b.len() && b[j] == b'}' && j > st && matches!(b[j - 1], b'e' | b'E') {
            if let Some(c) = src[st..j].find(':') {
                let sp = &src[st + c + 1..j - 1];
                if sp.bytes().all(|x| x.is_ascii_digit() || matches!(x, b'.' | b'+' | b'#' | b'<' | b'>' | b'^' | b'$')) {
                    return true;
                }
            }
        }
        i = st;
    }
    false
}

/// Whether some `vec![x; n]` has an x that is not a literal
fn vec_repeat_nonliteral(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find("vec![") {
        let start = i + k + 5;
        let mut depth = 0i32;
        let mut j = start;
        while j < b.len() {
            match b[j] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' if depth == 0 => break,
                b')' | b']' | b'}' => depth -= 1,
                b';' if depth == 0 => {
                    let x = src[start..j].trim_start();
                    let lit = x.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '\'')
                        || x.starts_with("true")
                        || x.starts_with("false")
                        || x.starts_with("b'");
                    if !lit {
                        return true;
                    }
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        i = start;
    }
    false
}

/// The prelude names that formatting uses
const FMT_NAMES: &[&str] = &[
    "String", "new", "Formatter", "Display", "Debug", "__exr_formatter", "__exr_fmt_spec",
    "__exr_fmt_lit", "__exr_fmt_int", "__exr_fmt_str", "__exr_fmt_bool", "__exr_fmt_char",
    "__exr_fmt_str_debug", "__exr_fmt_char_debug", "__exr_dbg_begin", "__exr_dbg_item",
    "__exr_dbg_item_end", "__exr_dbg_end", "__exr_print", "__exr_eprint", "__exr_display",
    "__exr_debug", "__exr_fmt_ok", "fmt", "__FmtError", "__exr_string_cap",
];

/// Whether a panic-like macro has format arguments (`panic!("bad {}", x)` and so on)
const PANIC_MACROS: [&str; 7] = ["panic!(", "unreachable!(", "todo!(", "unimplemented!(", "assert!(", "assert_eq!(", "assert_ne!("];

fn panic_with_args(src: &str) -> bool {
    PANIC_MACROS.iter().any(|m| {
        src.match_indices(m).any(|(i, _)| {
            let rest = &src[i + m.len()..];
            let line = rest.split('\n').next().unwrap_or("");
            line.contains('{') && line.contains("\",")
        })
    })
}

/// Whether `.to_string()` is called on something other than a string literal (`5.to_string()` uses formatting)
fn nonliteral_to_string(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find(".to_string()") {
        let at = i + k;
        if at == 0 || b[at - 1] != b'"' {
            return true;
        }
        i = at + 1;
    }
    false
}

/// When these names appear, the program is taken to use Iterator
const ITER_NAMES: &[&str] = &[
    "Iterator", "next", "map", "filter", "filter_map", "collect", "sum", "product", "count",
    "fold", "for_each", "any", "all", "find", "position", "rposition", "partition", "cycle", "nth", "zip", "chain", "take", "skip",
    "take_while", "copied", "cloned", "into_iter", "flat_map", "flatten", "step_by", "peekable", "skip_while", "scan",
    "inspect", "find_map", "unzip",
];

/// Adds a prelude item's identifiers to need (Iterator method names only in a method call's position)
fn add_idents(need: &mut crate::fxhash::FxSet<&'static str>, it: &SrcItem) {
    let bytes = PRELUDE.as_bytes();
    for &(a, b) in &it.idents {
        let w = &PRELUDE[a as usize..b as usize];
        if w != "Iterator" && ITER_NAMES.contains(&w) && !method_call_at(bytes, a as usize, b as usize) {
            continue;
        }
        if matches!(w, "width" | "precision" | "fill" | "sign_plus") && !method_call_at(bytes, a as usize, b as usize) {
            continue;
        }
        // (`__core_replace`, whose definition is looked up as `replace`)
        need.insert(w.strip_prefix("__core_").unwrap_or(w));
    }
}

fn bucket(first: u8, len: usize) -> usize {
    first as usize * 32 + len.min(31)
}

/// Whether h heads an `impl<T, U ..> PartialEq<..> for [T..]` (arrays / slices compared across types)
fn is_seq_eq_impl(h: usize) -> bool {
    let it = &items()[h].0;
    matches!(it.kind, ItemKind::ImplHeader)
        && PRELUDE[it.start..it.end].starts_with("impl<T, U")
        && PRELUDE[it.start..it.end].contains("PartialEq<")
}

/// The type's name, when h is the header of an impl for Vec / String
fn owner(h: usize) -> Option<&'static str> {
    static OWNERS: OnceLock<Vec<Option<&'static str>>> = OnceLock::new();
    OWNERS.get_or_init(|| {
        items()
            .iter()
            .map(|(it, _, _)| {
                if !matches!(it.kind, ItemKind::ImplHeader) {
                    return None;
                }
                // (`impl From<String> for PathBuf` belongs to PathBuf: with a `for`, look at the type after it)
                let ws: Vec<&'static str> = it.idents.iter().map(|&(a, b)| &PRELUDE[a as usize..b as usize]).collect();
                let from = ws.iter().position(|w| *w == "for").map(|i| i + 1).unwrap_or(0);
                // (when the type after it is no owner, as in `impl __PathExt for str`, by the trait's name. Nor is a
                // number type when the trait's arguments name an owner: `impl From<Ipv6Addr> for u128` comes with
                // Ipv6Addr, not with every program that names u128)
                let number = |w: &&str| matches!(*w, "u128" | "i128" | "f64" | "f32");
                let after: Vec<&str> = ws[from..].iter().copied().filter(|w| !(from > 0 && number(w))).collect();
                let numbers: Vec<&str> = ws[from..].iter().copied().filter(|w| from > 0 && number(w)).collect();
                after
                    .iter()
                    .chain(ws[..from].iter())
                    .chain(numbers.iter())
                    .copied()
                    .find_map(|w| match w {
                        "Vec" | "String" | "HashMap" | "Reverse" | "AssertUnwindSafe" | "LocalKey" | "LazyCell" | "BinaryHeap" | "PeekMut" | "SplitStr" | "RSplitStr" | "FlattenRef" | "MatchIndices" | "CharIndices"
                        | "Bytes" | "ToCase" | "CharEscape" | "ParseIntError" | "IntErrorKind" | "Rev" | "StepBy" | "Peekable"
                        | "SkipWhile" | "PreludeScan" | "Inspect" | "FlatMap" | "Flatten" | "Cloned" | "Windows" | "Chunks" | "ChunksExact" | "ChunksMut" | "Iter" | "IterMut" | "MaybeUninit"
                        | "HashSet" | "SetIntoIter" | "BTreeMap" | "BTreeSet" | "BtEntry" | "BtIter" | "BtIterMut"
                        | "BtIntoIter" | "VecDeque" | "DqIter" | "DqIntoIter" | "f64" | "f32" | "ParseFloatError" | "i128" | "u128" | "FromUtf8Error" | "Utf8Error" | "Rc" | "Arc" | "ManuallyDrop" | "Pin" | "NonNull" | "UnsafeCell" | "RefCell" | "Ref" | "RefMut" | "BorrowError" | "BorrowMutError" | "Cell" | "JoinHandle" | "__Sndr" | "__SyncSndr" | "__Rcvr__" | "__RecvIter" | "AtomicUsize" | "AtomicIsize" | "AtomicU8" | "AtomicU16" | "AtomicU32" | "AtomicU64" | "AtomicI8" | "AtomicI16" | "AtomicI32" | "AtomicI64" | "AtomicBool" | "OnceLock" | "OnceCell" | "Mutex" | "MutexGuard" | "RwLock" | "RwLockReadGuard" | "RwLockWriteGuard" | "SystemTime" | "SystemTimeError" | "Bound" | "Arguments" | "RangeFrom" | "TryFromIntError" | "TryFromSliceError" | "Stdin" | "StdinLock" | "StdinLines" | "Stdout" | "StdoutLock"
                        | "BufWriter" | "BufReader" | "Cursor" | "ChunksMut" | "IoError" | "__DynError" | "Args" | "DefaultHasher" | "VarError"
                        | "PathBuf" | "__PathExt" | "__AsPathStr" | "Command" | "__ExitStatus" | "Cow" | "__CowStr"
                        | "Duration" | "AtomicPtr" | "RangeTo" | "RangeToInclusive" | "Fuse" | "Wrapping" | "Saturating"
                        | "CString" | "CStr" | "NulError" | "__CStrBytes" | "LinkedList" | "Thread" | "Builder" | "__ThreadPanic" | "__TlsKey" | "AccessError" | "SendError" | "RecvError" | "TryRecvError" | "RecvTimeoutError" | "__RecvIntoIter" | "Barrier" | "BarrierWaitResult" | "Once" | "OnceState"
                        | "Layout" | "LayoutError" | "__LayoutAlign" | "Infallible" | "Condvar" | "ControlFlow"
                        | "Ipv4Addr" | "Ipv6Addr" | "IpAddr" | "SocketAddrV4" | "SocketAddrV6" | "SocketAddr" | "AddrParseError"
                        | "SplitNStr" | "SliceSplitN" | "PoisonError"
                        | "File" | "OpenOptions" | "Metadata" | "FileType" | "Permissions" | "FpCategory"
                        | "Instant" | "TryLockError" => Some(w),
                        // (std::num's NonZero types: their impls only with them)
                        w if w.starts_with("NonZero") => Some(w),
                        // HashMap's iterators (HmIter and so on) only when HashMap is used
                        w if w.starts_with("Hm") => Some("HashMap"),
                        _ => None,
                    })
            })
            .collect()
    })[h]
}

/// The name of `fn x` / `struct X` / `enum X` (`__core_x` is looked up as x)
fn defined_name(src: &str, it: &SrcItem) -> Option<String> {
    // (called for every prelude item at startup, so it walks the identifiers without building a Vec)
    let mut ids = it.idents.iter().map(|&(a, b)| &src[a as usize..b as usize]);
    // (a type alias `pub type Path = str;` too, but not an associated type `type Item` in an impl)
    let alias = matches!(it.kind, ItemKind::Normal);
    // (an associated const `const ZERO: Duration` too; for a `const fn`, the fn's name)
    ids.find(|w| matches!(*w, "fn" | "struct" | "enum" | "static" | "const") || (alias && *w == "type"))?;
    let mut n = ids.next()?;
    // (`const fn f`: f; `static mut X`: X, not `mut`, which almost every program names)
    if n == "fn" || n == "mut" {
        n = ids.next()?;
    }
    Some(n.strip_prefix("__core_").unwrap_or(n).to_string())
}

/// Returns, as source, only the prelude items the user's code may use (found by following identifiers).
/// A program that does not use Option gets nothing added, so the cache does not grow either
/// Prelude types a program may define for itself (no part of the compiler names them): when the program does, the
/// prelude's own is renamed `__p_<name>` in the prelude text, so the two do not collide
const SHADOWABLE: &[&str] = &[
    "Chain", "Entry", "Enumerate", "Filter", "FilterMap", "Map", "OptIter", "Scan", "Skip", "Take", "TakeWhile", "Zip",
    "Command", "Mutex", "VarError", "Rev", "StepBy", "Peekable", "Cloned", "Flatten", "FlatMap", "Inspect", "SkipWhile",
    "PathBuf", "Args", "BufWriter", "BufReader", "Pin", "Windows", "Bytes", "MatchIndices", "MutexGuard", "Ref", "RefMut", "BorrowError", "BorrowMutError", "Chunks", "Duration", "Fuse", "Empty", "RangeTo", "RangeToInclusive", "RangeFull",
    "AtomicPtr", "TryReserveError", "c_void", "Infallible", "Cycle", "JoinHandle", "Builder", "AccessError", "SendError", "RecvError", "TryRecvError", "RecvTimeoutError", "Barrier", "BarrierWaitResult", "Chars", "Poll", "Waker", "Context", "Future",
    "Wrapping", "Saturating", "NonZeroU8", "NonZeroU16", "NonZeroU32", "NonZeroU64", "NonZeroU128", "NonZeroUsize",
    "NonZeroI8", "NonZeroI16", "NonZeroI32", "NonZeroI64", "NonZeroI128", "NonZeroIsize",
    "FromStr", "CString", "CStr", "NulError", "LinkedList", "Ipv4Addr", "Ipv6Addr", "IpAddr", "SocketAddrV4", "SocketAddrV6",
    "SocketAddr", "AddrParseError", "RwLock", "SystemTime", "SystemTimeError", "Bound", "Arguments", "Thread", "ThreadId",
    "Once", "OnceState", "Layout", "LayoutError", "Condvar", "ControlFlow", "Repeat",
    "File", "OpenOptions", "Metadata", "FileType", "Permissions", "RawFd", "AsRawFd", "FromRawFd", "IntoRawFd",
    "MetadataExt", "OpenOptionsExt", "PermissionsExt", "FpCategory",
    "Instant", "Any",
    "Binary", "Octal", "LowerHex", "UpperHex", "LowerExp", "UpperExp", "Pointer",
    "LockResult", "TryLockResult", "TryLockError",
];

/// Whether the prelude's item of the name is renamed when the program defines one (SHADOWABLE, hashed: asked for each
/// type a program defines, 1200 times in a large one)
fn is_shadowable(w: &str) -> bool {
    static S: OnceLock<crate::fxhash::FxSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| SHADOWABLE.iter().copied().collect()).contains(w)
}

/// A name as it was before the compiler renamed it: `__u_Iter`, a program's type named as a prelude type
/// (rename_user_collisions), is `Iter`; `__p_ThreadId`, the prelude's renamed against a program's (rename_shadowed),
/// is `ThreadId`. Each renaming goes word by word, so a variant of such a name is renamed too. Any other name is
/// itself: a variant the program names `__p_Ready`
pub fn unrenamed(name: &str) -> &str {
    if let Some(n) = name.strip_prefix("__u_").filter(|n| prelude_type_names().contains(*n)) {
        return n;
    }
    if let Some(n) = name.strip_prefix("__p_").filter(|n| is_shadowable(n)) {
        return n;
    }
    name
}

/// The name a struct / enum / trait / union item defines (from its first identifiers: `pub struct Name`)
fn type_def_name<'a>(src: &'a str, it: &SrcItem) -> Option<&'a str> {
    if !matches!(it.kind, ItemKind::Normal | ItemKind::TraitHeader) {
        return None;
    }
    // (past the attributes and the visibility, however many words they have: `#[derive(Clone, Debug)] pub(crate)
    // struct Iter` is 6 words before its name; another item's keyword first means it defines no type)
    let mut ids = it.idents.iter().map(|&(a, b)| &src[a as usize..b as usize]);
    let is_ty = |w: &str| matches!(w, "struct" | "enum" | "trait" | "union");
    let other = |w: &str| matches!(w, "fn" | "impl" | "mod" | "const" | "static" | "type" | "use" | "macro_rules" | "extern");
    ids.find(|w| is_ty(w) || other(w)).filter(|w| is_ty(w))?;
    ids.next()
}

/// The names the prelude gives a struct / enum / trait / union (build.rs lists them)
fn prelude_type_names() -> &'static crate::fxhash::FxSet<&'static str> {
    static S: OnceLock<crate::fxhash::FxSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| PRELUDE_TYPE_NAMES.iter().copied().collect())
}

/// The program defines a struct / enum / trait / union under a name the prelude also uses for one, which the compiler
/// refers to by name (`Iterator`, `Iter`, `Add`, `Hasher` ..; the SHADOWABLE ones are renamed in the prelude
/// instead): the program's own is renamed `__u_<name>` in its source, wherever the name is not reached through a
/// `std::` / `core::` / `alloc::` path, nor in an attribute. Debug output, `type_name` and diagnostics drop the prefix
/// again. None if there is no such name. user_items: src split into items
pub fn rename_user_collisions(src: &str, user_items: &[SrcItem]) -> Option<String> {
    let mut hit: Vec<&str> = vec![];
    for it in user_items {
        if let Some(w) = type_def_name(src, it) {
            if prelude_type_names().contains(w) && !is_shadowable(w) && !hit.contains(&w) {
                hit.push(w);
            }
        }
    }
    if hit.is_empty() {
        return None;
    }
    // (then exactly, on the tokens: not in strings or comments, nor where the definition sits in a std path)
    let b = src.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let defined: Vec<&str> = toks
        .windows(2)
        .filter(|w| w[0].0 == 0 && w[1].0 == 0 && matches!(&src[w[0].1..w[0].2], "struct" | "enum" | "trait" | "union"))
        .map(|w| &src[w[1].1..w[1].2])
        .filter(|w| hit.contains(w))
        .collect();
    if defined.is_empty() {
        return None;
    }
    // (attributes keep their names: `#[derive(Copy)]` next to a `struct Copy;`)
    let mut in_attr = vec![false; toks.len()];
    let mut k = 0;
    while k < toks.len() {
        let p = |i: usize, c: &str| toks.get(i).is_some_and(|t| t.0 == 1 && &src[t.1..t.2] == c);
        if p(k, "#") && (p(k + 1, "[") || (p(k + 1, "!") && p(k + 2, "["))) {
            let mut j = if p(k + 1, "[") { k + 1 } else { k + 2 };
            let mut depth = 0;
            while j < toks.len() {
                if p(j, "[") {
                    depth += 1;
                } else if p(j, "]") {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                in_attr[j] = true;
                j += 1;
            }
            k = j + 1;
            continue;
        }
        k += 1;
    }
    let mut out = String::with_capacity(src.len() + 64);
    let mut at = 0;
    for (k, t) in toks.iter().enumerate() {
        if t.0 != 0 || in_attr[k] || !defined.contains(&&src[t.1..t.2]) {
            continue;
        }
        // (`std::iter::Iterator`: the path's first segment)
        let mut j = k;
        while j >= 3 && toks[j - 1].0 == 1 && toks[j - 2].0 == 1 && &src[toks[j - 2].1..toks[j - 1].2] == "::" && toks[j - 3].0 == 0 {
            j -= 3;
        }
        if j < k && matches!(&src[toks[j].1..toks[j].2], "std" | "core" | "alloc") {
            continue;
        }
        out.push_str(&src[at..t.1]);
        out.push_str("__u_");
        at = t.1;
    }
    out.push_str(&src[at..]);
    Some(out)
}

/// Where a forward scan of the program is: in code, a `//` comment, a `/* */` comment (its depth), a string, or a raw
/// string (its `#`s). rename_shadowed moves it only to a name that would count, so a program without one is not read
/// byte by byte
#[derive(Clone, Copy, PartialEq)]
enum LexAt {
    Code,
    Line,
    Block(u32),
    Str,
    Raw(usize),
}

/// The scan from (k, st) moved to i: what i is in (out of rename_shadowed's loop over every byte, which it would slow)
#[cold]
#[inline(never)]
fn lex_to(b: &[u8], k: &mut usize, st: &mut LexAt, i: usize) -> LexAt {
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while *k < i {
        let c = b[*k];
        match *st {
            LexAt::Line => {
                if c == b'\n' {
                    *st = LexAt::Code;
                }
            }
            LexAt::Block(d) => {
                if c == b'*' && b.get(*k + 1) == Some(&b'/') {
                    *st = if d == 1 { LexAt::Code } else { LexAt::Block(d - 1) };
                    *k += 1;
                } else if c == b'/' && b.get(*k + 1) == Some(&b'*') {
                    *st = LexAt::Block(d + 1);
                    *k += 1;
                }
            }
            LexAt::Str => {
                if c == b'\\' {
                    *k += 1;
                } else if c == b'"' {
                    *st = LexAt::Code;
                }
            }
            LexAt::Raw(h) => {
                if c == b'"' && *k + h < b.len() && b[*k + 1..=*k + h].iter().all(|&x| x == b'#') {
                    *st = LexAt::Code;
                    *k += h;
                }
            }
            LexAt::Code => {
                if c == b'/' && b.get(*k + 1) == Some(&b'/') {
                    *st = LexAt::Line;
                } else if c == b'/' && b.get(*k + 1) == Some(&b'*') {
                    *st = LexAt::Block(1);
                    *k += 1;
                } else if c == b'"' {
                    *st = LexAt::Str;
                } else if c == b'r'
                    && (*k == 0
                        || !is_id(b[*k - 1])
                        // (`br".."`, `cr#".."#`)
                        || (matches!(b[*k - 1], b'b' | b'c') && (*k == 1 || !is_id(b[*k - 2]))))
                    && matches!(b.get(*k + 1), Some(b'"' | b'#'))
                {
                    let mut j = *k + 1;
                    while j < b.len() && b[j] == b'#' {
                        j += 1;
                    }
                    if b.get(j) == Some(&b'"') {
                        *st = LexAt::Raw(j - *k - 1);
                        *k = j;
                    }
                } else if c == b'\'' {
                    // (a char literal, `'"'` or escaped `'\"'`, `'\''`: past it; a lifetime's quote stays)
                    if b.get(*k + 1) == Some(&b'\\') {
                        let mut j = *k + 3;
                        while j < b.len() && b[j] != b'\'' {
                            j += 1;
                        }
                        *k = j;
                    } else if b.get(*k + 2) == Some(&b'\'') {
                        *k += 2;
                    }
                }
            }
        }
        *k += 1;
    }
    *st
}

/// The prelude's type names (SHADOWABLE) the program defines for itself: one pass over the bytes, the name after
/// `struct ` / `enum ` / `trait ` / `type ` / `union ` (it replaces a search for each name: those were most of the cost
/// of a no-op build of a large program)
pub fn shadowed_defs(user_src: &str) -> Vec<&str> {
    let mut defined: Vec<&str> = vec![];
    let b = user_src.as_bytes();
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    // (the scan that tells comments and strings, moved forward only to a candidate: lex_to)
    let (mut lk, mut ls) = (0usize, LexAt::Code);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if matches!(c, b's' | b'e' | b't' | b'u') && (i == 0 || !is_id(b[i - 1])) {
            let rest = &b[i..];
            let kw = [&b"struct "[..], b"enum ", b"trait ", b"type ", b"union "].into_iter().find(|k| rest.starts_with(k));
            if let Some(k) = kw {
                let mut a = i + k.len();
                while a < b.len() && b[a] == b' ' {
                    a += 1;
                }
                let mut e = a;
                while e < b.len() && is_id(b[e]) {
                    e += 1;
                }
                let w = &user_src[a..e];
                // (not in a comment or a string: a doc comment's `# struct IpAddr;`, `#[doc = " # struct IpAddr;"]`
                // defines nothing. Looked at only for a name that would count)
                if is_shadowable(w) && !defined.contains(&w) && lex_to(b, &mut lk, &mut ls, i) == LexAt::Code {
                    defined.push(w);
                }
                i = e.max(i + 1);
                continue;
            }
        }
        i += 1;
    }
    defined
}

/// `defined`: shadowed_defs of the program's source
pub fn rename_shadowed(defined: &[&str], prelude: String) -> String {
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    if defined.is_empty() {
        return prelude;
    }
    let mut out = prelude;
    for &n in defined {
        if !out.contains(n) {
            continue;
        }
        let ob = out.as_bytes();
        let mut r = String::with_capacity(out.len() + 64);
        let mut i = 0;
        while let Some(k) = out[i..].find(n) {
            let at = i + k;
            let end = at + n.len();
            let word = (at == 0 || !is_id(ob[at - 1])) && (end == ob.len() || !is_id(ob[end]));
            r.push_str(&out[i..at]);
            if word {
                r.push_str("__p_");
            }
            r.push_str(n);
            i = end;
        }
        r.push_str(&out[i..]);
        out = r;
    }
    out
}

/// The program's std paths to a prelude type it shadows (a type of its own of the name, rename_shadowed):
/// `std::time::Duration` / `::core::time::Duration` / `use std::time::Duration as StdDuration;` name the prelude's,
/// renamed `__p_Duration`. Such a path becomes the renamed name, padded with spaces in front to the path's length, so
/// no column moves (`std::` alone is longer than `__p_`); in a `use`, and an item of a `use std::m::{..}` group, the
/// name is renamed in place, which moves that line's later columns by 4. Comments and strings are left alone
/// `defined`: shadowed_defs of the source (the caller's, which rename_shadowed takes as well: one scan)
pub fn std_paths_to_shadowed(user_src: &str, defined: &[&str]) -> Option<String> {
    if defined.is_empty() || !(user_src.contains("std::") || user_src.contains("core::") || user_src.contains("alloc::")) {
        return None;
    }
    let b = user_src.as_bytes();
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    // (an identifier at i: its end)
    let ident_end = |i: usize| {
        let mut e = i;
        while e < b.len() && is_id(b[e]) {
            e += 1;
        }
        e
    };
    let (mut lk, mut ls) = (0usize, LexAt::Code);
    // (byte ranges and what replaces them, in order)
    let mut reps: Vec<(usize, usize, String)> = vec![];
    let mut i = 0;
    while i < b.len() {
        let at_root = matches!(b[i], b's' | b'c' | b'a') && (i == 0 || !is_id(b[i - 1]));
        let root = if at_root { ["std::", "core::", "alloc::"].into_iter().find(|r| b[i..].starts_with(r.as_bytes())) } else { None };
        let Some(root) = root else {
            i += 1;
            continue;
        };
        if lex_to(b, &mut lk, &mut ls, i) != LexAt::Code {
            i += root.len();
            continue;
        }
        // (a leading `::`, `::std::..`, is part of the path)
        let start = if i >= 2 && &b[i - 2..i] == b"::" && (i == 2 || !is_id(b[i - 3])) { i - 2 } else { i };
        // (the modules: lowercase segments, each followed by `::`)
        let mut k = i + root.len();
        loop {
            let e = ident_end(k);
            if e > k && b[k].is_ascii_lowercase() && b[e..].starts_with(b"::") {
                k = e + 2;
            } else {
                break;
            }
        }
        if k < b.len() && b[k] == b'{' {
            // (a `use` group: its items that are such a name, `{Duration as StdDuration, Instant}`, one level deep)
            let mut j = k + 1;
            while j < b.len() && b[j] != b'}' && b[j] != b'{' && b[j] != b';' {
                if is_id(b[j]) && !is_id(b[j - 1]) {
                    let e = ident_end(j);
                    let item_start = b[k + 1..j].iter().rev().find(|c| !c.is_ascii_whitespace()).is_none_or(|&c| c == b',' || c == b'{');
                    if item_start && defined.contains(&&user_src[j..e]) && !b[e..].starts_with(b"::") {
                        reps.push((j, e, format!("__p_{}", &user_src[j..e])));
                    }
                    j = e;
                } else {
                    j += 1;
                }
            }
            i = j;
            continue;
        }
        let e = ident_end(k);
        if e > k && defined.contains(&&user_src[k..e]) {
            let to = format!("__p_{}", &user_src[k..e]);
            // (an import keeps its path, `use std::time::__p_Duration as StdDuration;`: an alias of a bare name would
            // not be read as one of the prelude's. It moves the line's later columns by 4)
            let mut w = start;
            while w > 0 && b[w - 1].is_ascii_whitespace() {
                w -= 1;
            }
            let in_use = w >= 3 && &b[w - 3..w] == b"use" && (w == 3 || !is_id(b[w - 4]));
            if in_use {
                reps.push((k, e, to));
            } else {
                let pad = (e - start).saturating_sub(to.len());
                reps.push((start, e, format!("{}{to}", " ".repeat(pad))));
            }
        }
        i = e.max(k).max(i + 1);
    }
    if reps.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(user_src.len() + 16);
    let mut last = 0;
    for (a, e, t) in reps {
        out.push_str(&user_src[last..a]);
        out.push_str(&t);
        last = e;
    }
    out.push_str(&user_src[last..]);
    Some(out)
}

/// Whether the source has a path `Box::from` in any spelling the parser takes: spaces around `::`, a turbofish
/// (`Box::<[u8]>::from`). Comments are not told apart (a match there only selects a few names more)
fn box_from_path(src: &str) -> bool {
    let b = src.as_bytes();
    // (white space and comments between the path's tokens, as the parser skips them: `Box /* c */ ::from`)
    let ws = |mut i: usize| {
        loop {
            if i >= b.len() {
                return b.len();
            }
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if b[i..].starts_with(b"//") {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            } else if b[i..].starts_with(b"/*") {
                let mut d = 0;
                while i < b.len() {
                    if b[i..].starts_with(b"/*") {
                        d += 1;
                        i += 2;
                    } else if b[i..].starts_with(b"*/") {
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
                return i;
            }
        }
    };
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let colons = |i: usize| (b.get(i) == Some(&b':') && b.get(i + 1) == Some(&b':')).then_some(i + 2);
    for (k, _) in src.match_indices("Box") {
        if k > 0 && word(b[k - 1]) || b.get(k + 3).is_some_and(|&c| word(c)) {
            continue;
        }
        let Some(mut i) = colons(ws(k + 3)) else { continue };
        i = ws(i);
        if b.get(i) == Some(&b'<') {
            // (to its closing `>`, past comments (`<[u8 /* > */]>`); none: no turbofish, no path)
            let mut d = 0;
            let mut closed = false;
            while i < b.len() {
                i = ws(i);
                match b.get(i) {
                    Some(b'<') => d += 1,
                    Some(b'>') => {
                        d -= 1;
                        if d == 0 {
                            closed = true;
                            break;
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
                i += 1;
            }
            if !closed {
                continue;
            }
            let Some(j) = colons(ws(i + 1)) else { continue };
            i = ws(j);
        }
        if b[i..].starts_with(b"from") && !b.get(i + 4).is_some_and(|&c| word(c)) {
            return true;
        }
    }
    false
}

pub fn select(user_src: &str, user_items: &[SrcItem]) -> String {
    select_with(user_src, user_items, &mut SelectCache::default())
}

/// What each item gave: the names it matched and whether it has an iterator chain (item hash → result).
/// An LSP / incremental build does not rescan the items that did not change
#[derive(Default)]
pub struct SelectCache {
    map: crate::fxhash::FxMap<crate::key::Key, (Vec<&'static str>, bool)>,
    /// (item hash, hash of the check) → whether the check holds on the item's text (PreludeScan)
    scan: crate::fxhash::FxMap<(crate::key::Key, u64), bool>,
    /// The number of user items in the last selection (the items after them are the prelude's)
    pub user_items: usize,
    /// The source has std paths std_paths_to_shadowed rewrote to `__p_Name`: such identifiers are looked at (asked
    /// of every identifier only then)
    pub shadow_paths: bool,
}

/// Checks each item instead of the whole user source (`contains` and so on), keeping results by item hash.
/// Text outside the items (such as a leading `#![no_std]`) is checked directly. LSP skips unchanged items
struct PreludeScan<'a> {
    src: &'a str,
    spans: Vec<(usize, usize, crate::key::Key)>,
    /// The text outside the items (joined with newlines)
    gaps: String,
    memo: &'a mut crate::fxhash::FxMap<(crate::key::Key, u64), bool>,
    /// where each byte that is not white space is (built at the first `has` of a large source)
    /// (lowercase letters too: an all-lowercase pattern (`stdout`, `dyn`) would otherwise read the whole source)
    idx: Option<ByteIdx>,
}

/// the positions of each indexed byte, in order (`start[b]..start[b + 1]` of `pos`)
struct ByteIdx {
    start: Vec<u32>,
    pos: Vec<u32>,
}

fn indexed_byte(b: u8) -> bool {
    !(b == b' ' || b == b'\n' || b == b'\t' || b >= 0x80)
}

impl ByteIdx {
    fn new(src: &[u8]) -> ByteIdx {
        let mut start = vec![0u32; 257];
        for &b in src {
            start[b as usize + 1] += 1;
        }
        for b in 0..256 {
            if !indexed_byte(b as u8) {
                start[b + 1] = 0;
            }
            start[b + 1] += start[b];
        }
        let mut pos = vec![0u32; start[256] as usize];
        let mut at = start.clone();
        for (i, &b) in src.iter().enumerate() {
            if indexed_byte(b) {
                pos[at[b as usize] as usize] = i as u32;
                at[b as usize] += 1;
            }
        }
        ByteIdx { start, pos }
    }

    /// src.contains(pat), looking only where pat's rarest indexed byte is (None: pat has no indexed byte)
    fn contains(&self, src: &[u8], pat: &[u8]) -> Option<bool> {
        let (j, b) = pat
            .iter()
            .enumerate()
            .filter(|(_, b)| indexed_byte(**b))
            .min_by_key(|(_, b)| self.start[**b as usize + 1] - self.start[**b as usize])?;
        let ps = &self.pos[self.start[*b as usize] as usize..self.start[*b as usize + 1] as usize];
        Some(ps.iter().any(|&p| {
            let p = p as usize;
            p >= j && src.len() - (p - j) >= pat.len() && &src[p - j..p - j + pat.len()] == pat
        }))
    }
}

impl<'a> PreludeScan<'a> {
    fn new(src: &'a str, user_items: &[SrcItem], memo: &'a mut crate::fxhash::FxMap<(crate::key::Key, u64), bool>) -> PreludeScan<'a> {
        let mut spans: Vec<(usize, usize, crate::key::Key)> = user_items.iter().map(|it| (it.start, it.end, it.hash)).collect();
        spans.sort_by_key(|x| x.0);
        let mut gaps = String::new();
        let mut at = 0;
        for &(a, b, _) in &spans {
            if a > at && !src[at..a].trim().is_empty() {
                gaps.push_str(&src[at..a]);
                gaps.push('\n');
            }
            at = at.max(b);
        }
        if at < src.len() {
            gaps.push_str(&src[at..]);
        }
        // (drop the old records when too many pile up)
        if memo.len() > 1 << 20 {
            memo.clear();
        }
        PreludeScan { src, spans, gaps, memo, idx: None }
    }

    /// any, when f can be true only where one of `pre` is in the source (looked up in the byte index first: most of
    /// these are in no source, and f reads all of it)
    fn any_pre(&mut self, key: &str, pre: &[&str], f: &dyn Fn(&str) -> bool) -> bool {
        if self.src.len() >= 1 << 18 && !pre.iter().any(|p| self.has(p)) {
            return false;
        }
        self.any(key, f)
    }

    /// Whether f is true on some item (or outside the items). key is f's name (the key the results are kept by)
    fn any(&mut self, key: &str, f: &dyn Fn(&str) -> bool) -> bool {
        // (one look at the whole source, as `has` does: running f on every item was 12% of a no-op build of a
        // large program, and the memo only helps an LSP edit)
        if self.spans.len() > 64 {
            return f(self.src);
        }
        let mut kh: u64 = 0xcbf2_9ce4_8422_2325;
        for &c in key.as_bytes() {
            kh = (kh ^ c as u64).wrapping_mul(0x0100_0000_01b3);
        }
        if !self.gaps.is_empty() && f(&self.gaps) {
            return true;
        }
        for &(a, b, h) in &self.spans {
            let hit = match self.memo.get(&(h, kh)) {
                Some(&x) => x,
                None => {
                    let x = f(&self.src[a..b]);
                    self.memo.insert((h, kh), x);
                    x
                }
            };
            if hit {
                return true;
            }
        }
        false
    }

    /// (the same as asking every item and the text between them, except for a match across an item boundary: one
    /// search of the whole source is cheaper than a memo per item, even when only one item changed)
    /// (a large source is searched through its byte index: most patterns are not there, and each such search would
    /// read the whole source)
    fn has(&mut self, pat: &str) -> bool {
        if self.src.len() < 1 << 18 {
            return self.src.contains(pat);
        }
        let src = self.src.as_bytes();
        let idx = self.idx.get_or_insert_with(|| ByteIdx::new(src));
        match idx.contains(src, pat.as_bytes()) {
            Some(x) => x,
            None => self.src.contains(pat),
        }
    }
}

/// Whether src writes a C string literal, `c"..."` / `cr"..."`: a `c` that starts a token (not the end of `"abc"` or of a
/// name)
/// Whether the source has a format spec ending in `p`: `:` with fill / flags / width (`#`, `018`, `*>20`, `>w$`) between
/// it and the `p`, then `}` (`{:p}`, `{pointer:#p}`), as a format string writes one. Only at the source's `p}`s, looked
/// up first (the byte index of a large source): most sources have none
fn has_ptr_spec(sc: &mut PreludeScan) -> bool {
    if !sc.has("p}") {
        return false;
    }
    let b = sc.src.as_bytes();
    sc.src.match_indices("p}").any(|(at, _)| {
        // (back to the spec's `:`, over what a spec may hold; a `{` / `}` / quote / line break first means it is not
        // one. A `:` with another before it is the fill of `{::>8p}` when an alignment follows it, else a path's `::`)
        let lo = at.saturating_sub(128);
        let mut j = at;
        while j > lo {
            j -= 1;
            match b[j] {
                b'{' | b'}' | b'"' | b'\n' => return false,
                b':' if j > 0 && b[j - 1] == b':' && !matches!(b.get(j + 1), Some(b'<' | b'^' | b'>')) => return false,
                b':' => return j > 0 && (matches!(b[j - 1], b'{' | b':' | b'_') || b[j - 1].is_ascii_alphanumeric()),
                _ => {}
            }
        }
        false
    })
}

fn has_cstr_literal(src: &str) -> bool {
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(k) = src[i..].find('c') {
        let at = i + k;
        let starts = at == 0 || !(b[at - 1].is_ascii_alphanumeric() || matches!(b[at - 1], b'_' | b'"' | b'\''));
        let rest = &b[at + 1..];
        if starts && (rest.first() == Some(&b'"') || (rest.first() == Some(&b'r') && matches!(rest.get(1), Some(b'"' | b'#')))) {
            return true;
        }
        i = at + 1;
    }
    false
}

/// A float literal such as `1.5` (a `.` between digits) outside string literals: the `.` and `"` of the source are
/// gone through with memchr (`str::find` of a char), not byte by byte, as a large program has none of the one and few
/// of the other (a quote after `\\` or `'` does not open or close a string)
fn float_lit_in(src: &str) -> bool {
    let b = src.as_bytes();
    if b.len() < 3 {
        return false;
    }
    // (the byte at 0 and the last are not looked at, as by the byte loop this replaces. `from` is 0 or just after an
    // ASCII `"` / `.`: a char boundary)
    let end = b.len() - 1;
    let next = |from: usize, c: char| match src[from..].find(c) {
        Some(k) if from + k < end => from + k,
        _ => usize::MAX,
    };
    let first = |c: char| match next(0, c) {
        0 => next(1, c),
        k => k,
    };
    let mut in_str = false;
    let (mut q, mut d) = (first('"'), first('.'));
    loop {
        if q < d {
            if b[q - 1] != b'\\' && b[q - 1] != b'\'' {
                in_str = !in_str;
            }
            q = next(q + 1, '"');
        } else if d != usize::MAX {
            if !in_str && b[d - 1].is_ascii_digit() && b[d + 1].is_ascii_digit() {
                return true;
            }
            d = next(d + 1, '.');
        } else {
            return false;
        }
    }
}

pub fn select_with(user_src: &str, user_items: &[SrcItem], cache: &mut SelectCache) -> String {
    let all = items();
    // Collect only the user's identifiers that match a name the prelude defines (or a related name).
    // Building a set of every identifier costs ms per build on a large program, so look up in a small set
    // Compare directly with the few candidates per first character: no hashing, so large programs stay fast
    static NAMES: OnceLock<Vec<Vec<&'static str>>> = OnceLock::new();
    let by_first = NAMES.get_or_init(|| {
        let mut h: Vec<&'static str> = all.iter().filter_map(|x| x.1.as_deref()).collect();
        // `s.to_uppercase()` / `c.is_alphabetic()` are looked up by the `__exr_str_*` / `__exr_char_*` helper names
        h.extend(helper_suffixes().keys().copied());
        h.extend([
            "Some", "None", "Ok", "Err", "core", "std", "Box", "dyn", "Drop", "vec", "to_string",
            "to_owned", "iter", "iter_mut", "chars", "into_iter", "next", "map", "filter",
            "filter_map", "collect", "sum", "product", "count", "fold", "for_each", "any", "all",
            "find", "position", "nth", "zip", "chain", "take", "skip", "take_while", "copied",
            "cloned", "format", "print", "println", "eprint", "eprintln", "write", "writeln",
            "split", "split_whitespace", "lines", "trim", "starts_with", "ends_with", "contains",
            "clamp", "from_u32", "char", "join", "concat", "drain", "default", "f64", "f32", "i128", "u128", "Iterator", "into", "split_at_mut",
            "copy_from_slice", "clone_from_slice", "BufRead", "Read", "Seek", "SeekFrom", "Write", "__FWr", "Cursor", "ExactSizeIterator", "BuildHasher", "split_first_chunk", "first_chunk", "chunks_exact", "chunks_mut", "chunks_exact_mut", "is_sorted", "is_sorted_by", "is_sorted_by_key",
            "split_first", "split_last", "splitn",
            // (the trait `Any`, a TraitHeader and so with no name of its own: `Box<dyn Any + Send>` alone needs it)
            "Any",
            // (the fmt traits of `{:b}` / `{:x}` / `{:e}` .., TraitHeaders and so with no names of their own)
            "Binary", "Octal", "LowerHex", "UpperHex", "LowerExp", "UpperExp", "Pointer",
        ]);
        // Candidates per (first byte, length): however many prelude names there are, 0 or 1 comparisons per identifier
        let mut t: Vec<Vec<&'static str>> = vec![vec![]; 256 * 32];
        for n in h {
            let k = bucket(n.as_bytes()[0], n.len());
            if !t[k].contains(&n) {
                t[k].push(n);
            }
        }
        t
    });
    // (Fx hashing: the set is asked thousands of times per build)
    let mut need: crate::fxhash::FxSet<&str> = Default::default();
    let bytes = user_src.as_bytes();
    let mut chain = false;
    let shadow_paths = cache.shadow_paths;
    let old = std::mem::take(&mut cache.map);
    for it in user_items {
        if let Some(r) = old.get(&it.hash) {
            need.extend(r.0.iter().copied());
            chain |= r.1;
            cache.map.insert(it.hash, r.clone());
            continue;
        }
        let mut names: Vec<&'static str> = vec![];
        // (`__p_Duration`, a std path to a prelude type the program shadows (std_paths_to_shadowed): the prelude's
        // `Duration`, which the program's own name alone does not bring. A loop of its own, only for such a source)
        if shadow_paths {
            for &(a, b) in &it.idents {
                if let Some(w) = user_src[a as usize..b as usize].strip_prefix("__p_").filter(|w| !w.is_empty()) {
                    if let Some(n) = by_first[bucket(w.as_bytes()[0], w.len())].iter().find(|n| **n == w) {
                        names.push(n);
                    }
                }
            }
        }
        for &(a, b) in &it.idents {
            let cands = &by_first[bucket(bytes[a as usize], (b - a) as usize)];
            if cands.is_empty() {
                continue;
            }
            let w = &user_src[a as usize..b as usize];
            if let Some(n) = cands.iter().find(|n| **n == w) {
                // An Iterator method name counts only in a method call's position (`.count()`)
                // (a local variable `count` or `sum` does not add Iterator)
                // (the last segment of a path such as `mem::take(..)` counts)
                let after_path = a >= 2 && &bytes[a as usize - 2..a as usize] == b"::";
                // (Formatter's `width()` and so on, too, only as a method call: a field or variable `width` adds nothing)
                if matches!(*n, "width" | "precision" | "fill" | "sign_plus") && !method_call_at(bytes, a as usize, b as usize) {
                    continue;
                }
                if *n != "Iterator" && ITER_NAMES.contains(n) && !method_call_at(bytes, a as usize, b as usize) && !after_path {
                    continue;
                }
                // A formatting macro only as `name!(` (the method `p.write(..)` is something else)
                if matches!(*n, "format" | "print" | "println" | "eprint" | "eprintln" | "write" | "writeln")
                    && bytes[b as usize..].iter().find(|c| !c.is_ascii_whitespace()) != Some(&b'!')
                {
                    continue;
                }
                if *n == "cmp" && bytes.get(b as usize) == Some(&b':') {
                    continue;
                }
                // (the program's own function `clamp(v, lo, hi)` does not add it)
                if *n == "clamp" && !method_call_at(bytes, a as usize, b as usize) {
                    continue;
                }
                if !names.contains(n) {
                    names.push(n);
                }
            }
        }
        // An iterator chain appears only inside an item
        let c = !names.is_empty() && iter_chain(&user_src[it.start..it.end]);
        need.extend(names.iter().copied());
        chain |= c;
        cache.map.insert(it.hash, (names, c));
    }
    let mut sc = PreludeScan::new(user_src, user_items, &mut cache.scan);
    // (a program's write! into a Formatter, which its impl of a fmt trait names: the Formatter of its own that codegen
    // gives it. Asked of the program's names alone, before any rule adds the formatting names, Formatter among them)
    if need.contains("Formatter") {
        need.insert("__exr_sub_formatter");
    }
    // (`c"..."`: codegen makes it a call of __exr_cstr_lit, a `&'static CStr`; before the `no_std` return, which a
    //  program needing nothing else takes)
    if has_cstr_literal(user_src) {
        need.insert("__exr_cstr_lit");
        need.insert("CStr");
    }
    // (a std program always uses the panic handler, so it gets the prelude even when it uses nothing)
    if need.is_empty() && sc.has("no_std") {
        return String::new();
    }
    // (Duration is not in std's prelude: without `time` written (`use std::time::Duration` / `time::Duration`),
    // `Duration` is the program's own or an error)
    // (a std path to it the program's own `Duration` shadows is `__p_Duration` by now (std_paths_to_shadowed): the
    // prelude's, though `time` is gone from the text)
    if need.contains("Duration") && !sc.has("time") && !sc.has("__p_Duration") {
        need.remove("Duration");
    }
    // (`iter::Empty` likewise: `Empty` alone is usually a variant of the program's own enum)
    if need.contains("Empty") && !sc.has("iter::") && !sc.has("__p_Empty") {
        need.remove("Empty");
    }
    if need.contains("Repeat") && !sc.has("iter::") && !sc.has("__p_Repeat") {
        need.remove("Repeat");
    }
    // (`a.zip(&v)` / `a.chain([x])`: codegen makes Vec / array arguments `.iter()` / `.into_iter()`; `a.zip(dst)` with
    //  dst: &mut [T] is its `.iter_mut()`)
    if need.contains("zip") || need.contains("chain") {
        need.insert("into_iter");
        need.insert("iter");
        need.insert("iter_mut");
    }
    // (`arr.into_iter()` on an array variable is `arr.iter().copied()`, which codegen writes without the name)
    if need.contains("into_iter") {
        need.insert("copied");
    }
    // Formatting macros: Formatter and the output functions (codegen calls them; the source never names them)
    if ["format", "print", "println", "eprint", "eprintln", "write", "writeln", "fmt", "Formatter"]
        .iter()
        .any(|n| need.contains(n))
        || sc.any_pre("nonliteral_to_string", &[".to_string()"], &nonliteral_to_string)
        || sc.any_pre("panic_with_args", &PANIC_MACROS, &panic_with_args)
        // (a failing assert_eq! / assert_ne! prints left / right with {:?}; no_std only calls the panic_handler)
        || ((sc.has("assert_eq!(") || sc.has("assert_ne!(")) && !sc.has("no_std"))
    {
        need.extend(FMT_NAMES.iter().copied());
        if sc.has("{:?}") || sc.has("assert_eq!") || sc.has("assert_ne!") || sc.has("dbg!") {
            need.insert("__range_debug");
        }
        // (`{:?}` of a raw pointer)
        if sc.has("*const") || sc.has("*mut") {
            need.insert("__exr_fmt_ptr");
        }
        // (`{:p}`, `{pointer:p}`: a thin pointer's address, a slice's / dyn's `Pointer { addr, metadata }`)
        if has_ptr_spec(&mut sc) {
            need.extend(["__exr_fmt_ptr", "__exr_fmt_fat_ptr", "__exr_fmt_dyn_ptr"]);
        }
    }
    // `vec![..]` becomes Vec::with_capacity + push, and `"..".to_string()` becomes String::from
    if need.contains("vec") {
        need.extend(["Vec", "with_capacity", "push", "__exr_memset"]);
    }
    if need.contains("to_string") || need.contains("to_owned") {
        need.extend(["String", "from"]);
    }
    // Add the iterator parts only when Iterator (an adapter or a consuming method) is used.
    // A bare `for x in v.iter()` needs none: codegen loops over it directly
    // (iterator chains always start with a producer (iter / into_iter / chars / a range / a user impl Iterator).
    //  Option::map or a fold on the program's own type alone does not add them)
    let producer = ["iter", "iter_mut", "into_iter"].iter().any(|n| need.contains(n));
    if sc.has("ptr::eq") {
        need.insert("__exr_ptr_eq");
    }
    // (`__core_ptr_swap`, whose definition is looked up as `ptr_swap`)
    if sc.has("ptr::swap") {
        need.insert("ptr_swap");
    }
    // (`__core_slice_from_ref` / `_mut`, looked up as `slice_from_ref` / `slice_from_mut`)
    for (path, f) in [("slice::from_ref", "slice_from_ref"), ("slice::from_mut", "slice_from_mut")] {
        if sc.has(path) {
            need.insert(f);
        }
    }
    for (path, f) in [("Ref::map", "__exr_ref_map"), ("RefMut::map", "__exr_refmut_map"), ("Ref::clone", "__exr_ref_clone")] {
        if sc.has(path) {
            need.extend([f, "__core_forget", "forget"]);
        }
    }
    if sc.has("array::from_fn") {
        need.insert("__exr_array_from_fn");
    }
    // (`Box::from(s)` of a slice is `s.to_vec().into_boxed_slice()`)
    if sc.has("Box") && box_from_path(sc.src) {
        need.extend(["to_vec", "__exr_slice_to_vec", "Vec", "into_boxed_slice"]);
    }
    if sc.has("Cow") {
        need.extend(["Cow", "__CowStr", "Deref", "deref", "into_owned", "to_vec", "__exr_slice_to_vec", "Vec", "String"]);
    }
    // std::path / std::process::Command / env::var_os / split_paths / fs::remove_file
    if ["std::path", "PathBuf", "Path::", "&Path", "process::Command", "Command::new", "var_os", "split_paths", "remove_file", "OsStr"]
        .iter()
        .any(|n| sc.has(n))
    {
        need.extend([
            "Path", "OsStr", "OsString", "PathBuf", "__PathExt", "__AsPathStr", "__AsBytes", "__exr_path_new", "__exr_path_mode",
            "__exr_fs_remove_file", "__exr_env_var_os", "__exr_env_split_paths", "IntoIter", "into_iter", "Deref", "deref",
            "Command", "__ExitStatus", "__ProcOutput", "IoError", "Result", "Option", "String", "Vec", "Display", "Debug", "push", "split",
            "SplitStr", "Iterator", "next", "trim_end_matches", "rfind", "truncate", "starts_with", "ends_with", "push_str",
            "IntoIterator", "extend_from_slice", "file_stem", "set_extension", "__exr_c_path", "__exr_cstr",
        ]);
    }
    // The &str split / lines and so on are prelude functions (ones returning an iterator need its parts too)
    for (m, f) in [
        ("split", "__exr_str_split"),
        ("split_whitespace", "__exr_str_split_ws"),
        ("split_ascii_whitespace", "__exr_str_split_ascii_whitespace"),
        ("lines", "__exr_str_lines"),
        ("trim", "__exr_str_trim"),
        ("starts_with", "__exr_str_starts_with"),
        ("ends_with", "__exr_str_ends_with"),
        ("contains", "__exr_str_find"),
        ("find", "__exr_str_find"),
    ] {
        if need.contains(m) {
            need.insert(f);
            if !matches!(m, "split" | "split_whitespace" | "split_ascii_whitespace" | "lines" | "trim") {
                need.insert("__exr_char_utf8");
            }
        }
    }
    if need.contains("HashSet") {
        need.extend(["HashMap", "keys", "into_iter", "contains_key", "next"]);
    }
    if need.contains("HashMap") {
        need.extend(["__exr_hash", "__exr_hash_bytes"]);
    }
    if need.contains("BTreeMap") || need.contains("BTreeSet") {
        need.extend(["__exr_cmp", "Vec", "insert", "remove", "next"]);
    }
    if need.contains("VecDeque") {
        need.extend(["next", "Drop", "index", "index_mut"]);
    }
    if need.contains("BTreeMap") {
        need.insert("index");
    }
    // `s + "x"` / `s += "x"`: String's add / add_assign (operators do not appear as identifiers)
    if need.contains("String") || need.contains("to_string") || need.contains("format") {
        need.extend(["add", "add_assign"]);
    }
    // The &str / String / char helpers (`s.replace(..)` → __exr_str_replace)
    let hs = helper_suffixes();
    // (for a method name that integers, Option or iterators share, only when the program seems to use strings:
    //  String appears, or a string literal is the receiver (`"ab".repeat(3)`))
    // (or a `&str` / `&'static str` written: `NAME.to_ascii_lowercase()` with `static NAME: &'static str`)
    let mut str_evidence = |n: &str| -> bool {
        need.contains("String") || sc.has(&format!("\".{n}("))
    };
    let found: Vec<&'static str> = need
        .iter()
        .filter(|n| {
            !matches!(**n, "to_ascii_uppercase" | "to_ascii_lowercase" | "eq_ignore_ascii_case" | "replace" | "repeat")
                || str_evidence(n)
        })
        .filter_map(|n| hs.get(n))
        .flatten()
        .copied()
        .collect();
    // (helpers that loop over chars and helpers that return an iterator also need the iterator parts)
    let iter_helper = found.iter().any(|f| {
        matches!(
            *f,
            "__exr_str_char_indices" | "__exr_str_bytes" | "__exr_char_to_uppercase" | "__exr_char_to_lowercase" | "__exr_char_escape_unicode" | "__exr_char_escape_default"
        )
    });
    // (codegen turns a char passed for a helper's &str argument into UTF-8)
    if found.iter().any(|f| f.starts_with("__exr_str_")) {
        need.insert("__exr_char_utf8");
    }
    // (a helper giving a String: the String it is, and its `&*s` / `as_str` (`&*NAME.to_ascii_lowercase()`))
    if found.iter().any(|f| matches!(*f, "__exr_str_to_ascii_lowercase" | "__exr_str_to_ascii_uppercase" | "__exr_str_to_lowercase" | "__exr_str_to_uppercase")) {
        need.extend(["String", "as_str"]);
    }
    need.extend(found);
    if iter_helper {
        need.extend(["chars", "Iterator", "next"]);
    }
    // `s.split("ab")` (a separator that is not a char)
    if need.contains("split")
        && sc.any_pre("split_str", &[".split("], &|user_src: &str| user_src.match_indices(".split(").any(|(i, m)| user_src.as_bytes().get(i + m.len()) != Some(&b'\'')))
    {
        need.extend(["__exr_str_split_str", "Iterator"]);
    }
    // (`s.split_ascii_whitespace().collect()`: Split's next)
    if need.contains("split_ascii_whitespace") {
        need.extend(["Iterator", "next"]);
    }
    // `s.rsplit("::")` (an iterator that splits from the end)
    if need.contains("rsplit") {
        need.extend(["__exr_str_rsplit", "Iterator", "next"]);
    }
    if need.contains("match_indices") {
        need.extend(["__exr_str_match_indices", "Iterator", "next"]);
    }
    if need.contains("from") && need.contains("char") {
        need.insert("__exr_char_from_u8");
    }
    if need.contains("from_u32") {
        need.insert("__exr_char_from_u32_checked");
    }
    if need.contains("from_u32_unchecked") {
        need.insert("__exr_char_from_u32_unchecked");
    }
    // (the program's own `Vec::from_raw_parts`: the prelude's code names the slice's)
    if sc.has("from_raw_parts") && sc.has("Vec") {
        need.insert("__exr_vec_from_raw_parts");
    }
    if sc.has("from_utf8_unchecked") && sc.has("String") {
        need.insert("__exr_string_from_utf8_unchecked");
    }
    if need.contains("parse") {
        need.extend(["from_str", "FromStr"]);
    }
    if need.contains("flat_map") || need.contains("flatten") {
        need.extend(["__flat_prep", "__flat_next", "__Flat", "__flat_mark", "reverse", "pop", "take"]);
    }
    // (flatten over a sequence of references becomes `flat_map(|x| x.iter())`)
    if need.contains("flatten") {
        need.extend(["__exr_flatten_ref", "FlattenRef", "__exr_slice_iter", "Iter"]);
    }
    // `T::default()` / `Default::default()` / `..Default::default()` / unwrap_or_default / mem::take
    if ["default", "Default", "unwrap_or_default", "take"].iter().any(|n| need.contains(n)) {
        need.insert("__exr_default");
    }
    // Codegen turns `x.clone()` into __exr_clone::<T> (also inside the prelude's Vec::clone / cloned / to_vec, and a
    // slice's to_owned, which is its to_vec)
    // (not needed for types without drop, or for types with their own clone (String / Vec))
    if ["clone", "cloned", "to_vec", "to_owned", "resize"].iter().any(|n| need.contains(n)) {
        need.insert("__exr_clone");
    }
    if need.contains("join") || need.contains("concat") {
        need.extend(["__exr_join_str", "__exr_join_string"]);
        if need.contains("concat") {
            need.insert("__exr_concat_vec");
        }
    }
    if need.contains("drain") {
        need.insert("__drain");
    }
    // contains on arrays / slices (a prelude function that compares by PartialEq unless the elements are integers)
    if need.contains("contains") {
        need.insert("__exr_slice_contains");
    }
    for (m, f) in [
        ("windows", "__exr_slice_windows"),
        ("is_sorted", "__exr_slice_is_sorted"),
        ("is_sorted_by", "__exr_slice_is_sorted_by"),
        ("is_sorted_by_key", "__exr_slice_is_sorted_by_key"),
        ("chunks", "__exr_slice_chunks"),
        ("chunks_exact", "__exr_slice_chunks_exact"),
        ("chunks_mut", "__exr_slice_chunks_mut"),
        ("chunks_exact_mut", "__exr_slice_chunks_exact_mut"),
        ("split_first_chunk", "__exr_slice_split_first_chunk"),
        ("first_chunk", "__exr_slice_first_chunk"),
        ("to_vec", "__exr_slice_to_vec"),
        ("copy_from_slice", "__exr_slice_copy_from"),
        ("clone_from_slice", "__exr_slice_clone_from"),
        ("split_at", "__exr_slice_split_at"),
        ("split_at_mut", "__exr_slice_split_at_mut"),
        ("splitn", "__exr_slice_splitn"),
    ] {
        if need.contains(m) {
            need.extend([f, "Iterator", "next"]);
        }
    }
    // (`s.split_first()` / `s.split_last()`, and a slice's `to_owned()`, its to_vec: no iterator (#256))
    for (m, f) in [("split_first", "__exr_slice_split_first"), ("split_last", "__exr_slice_split_last"), ("to_owned", "__exr_slice_to_vec")] {
        if need.contains(m) {
            need.insert(f);
        }
    }
    // checked_* return Option
    if sc.has("checked_") || sc.has("overflowing_") {
        need.insert("Option");
    }
    // In `vec![x; n]` with an x that seems not to be Copy (not a literal), fill with resize
    if need.contains("vec") && sc.any_pre("vec_repeat_nonliteral", &["vec!["], &vec_repeat_nonliteral) {
        need.extend(["resize", "new", "__exr_clone"]);
    }
    // (`&*s` / `&**s` / `&s[..]` with s a String / &String: acg rewrites it to `s.as_str()`)
    if need.contains("String") && (sc.has("&*") || sc.has("[..]")) {
        need.insert("as_str");
    }
    // (a str trait's method called on a String, `s.custom_len()` with `impl Ext for str`, or an OsString's
    // to_string_lossy: acg calls it on `s.as_str()` (#256))
    if need.contains("String") && (need.contains("to_string_lossy") || sc.has("for str")) {
        need.insert("as_str");
    }
    // A collect into String uses push_str when the elements are &str / String
    if need.contains("collect") && need.contains("String") {
        need.extend(["push_str", "as_str"]);
    }
    // Rc calls the methods of its deref target
    if need.contains("Rc") {
        need.insert("deref");
    }
    // `char::is_ascii_digit` as a function value
    if sc.has("char::is_ascii") || sc.has("char::to_ascii") {
        for (_, name, _) in items() {
            if let Some(n) = name.as_deref() {
                if let Some(m) = n.strip_prefix("__exr_charfn_") {
                    if sc.has(&format!("char::{m}")) {
                        need.insert(n);
                    }
                }
            }
        }
    }
    // `==` on a type recursive through Box goes through a glue function (expanding it in place never stops)
    if (need.contains("Box") || sc.has("&'")) && sc.has("PartialEq") {
        need.insert("__exr_eq");
    }
    if need.contains("BTreeMap") && sc.has(".range(") {
        need.extend(["range", "__bt_span", "Range", "RangeInclusive"]);
        if sc.any_pre("open_range", &["..)"], &open_range) || sc.has("..))") {
            need.insert("RangeFrom");
        }
    }
    if need.contains("rotate_left") || need.contains("rotate_right") {
        need.extend(["rotate_left", "__rev_range", "swap"]);
    }
    if sc.has(".or_default()") {
        need.extend(["or_default", "__exr_default"]);
    }
    // (the entries of Formatter's builders take an iterator)
    if sc.has(".entries(") {
        need.extend(["Iterator", "iter", "next", "into_iter"]);
    }
    // `HashMap::from([..])` becomes vec![..]
    if sc.has("::from([") {
        need.extend(["from", "vec", "reverse", "pop", "insert"]);
    }
    if need.contains("extend") && need.contains("String") {
        need.extend(["push_str", "as_str", "push"]);
    }
    if sc.has("].into_iter()") {
        need.extend(["vec", "into_iter", "IntoIter", "Iterator", "next"]);
    }
    if need.contains("cycle") {
        need.extend(["Cycle", "__exr_clone"]);
    }
    // The panic of the arithmetic checks (a no_std program with a panic_handler does not use it)
    if !sc.has("no_std") {
        need.extend(["__exr_err_str", "__exr_fflush", "__exr_write", "__exr_exit"]);
        need.extend(["__exr_rt_init", "__exr_on_trap", "__exr_sigaction", "__EXR_TRAP_TABLE", "__EXR_KIND_MSG", "__EXR_KIND_OFF", "__EXR_SA", "__exr_err_num", "__exr_panic_index", "__exr_tab_uv", "__EXR_CALLER", "__exr_tab_file", "__exr_panic_head_at", "__exr_trap_search"]);

    }
    // (Duration's as_millis / as_micros / as_nanos return u128)
    if need.contains("Duration") && (need.contains("as_millis") || need.contains("as_micros") || need.contains("as_nanos")) {
        need.insert("u128");
    }
    // (the suffix of `42u128` does not appear as an identifier)
    for t in ["u128", "i128"] {
        if sc.has(t) {
            need.insert(t);
        }
    }
    if sc.has("fs::") {
        need.extend(["__exr_fs_read", "__exr_fs_read_to_string", "__exr_fs_write", "IoError", "Result", "String", "__AsBytes", "Debug", "Display"]);
    }
    if sc.has("env::") {
        need.extend(["__exr_env_args", "Args", "__exr_env_var", "VarError", "Iterator", "next", "String", "__AsBytes", "Result", "Option"]);
    }
    if sc.has("process::exit") {
        need.insert("process_exit");
    }
    if sc.has("dbg!") {
        need.extend(FMT_NAMES.iter().copied());
        need.extend(["eprintln", "Debug", "__exr_eprint"]);
    }
    // (`ptr::write(p, v)` / `ptr::read(p)`: `write` / `read` alone are not looked up as functions; one scan for
    // `ptr::` first: most programs have none)
    if sc.has("ptr::") {
        for (pat, n) in [("ptr::write(", "write"), ("ptr::read(", "read"), ("ptr::drop_in_place(", "drop_in_place"), ("read_unaligned(", "read_unaligned"), ("write_unaligned(", "write_unaligned")] {
            if sc.has(pat) {
                need.insert(n);
            }
        }
    }
    // (`iter::repeat(x).take(n)`: the iterator these return needs the trait, even with no `next` written; only
    // with `iter::` written: `is_empty` / `call_once` are not these)
    if sc.has("iter::") && (sc.has("repeat") || sc.has("once") || sc.has("empty") || sc.has("successors")) {
        need.extend(["Iterator", "next"]);
    }
    if sc.has("dyn") && (sc.has("Display") || sc.has("Debug")) {
        need.extend(["__exr_dyn_display", "__exr_dyn_debug"]);
    }
    // (`x.fmt(f)` / `fmt::Debug::fmt(s, f)` of a value with no fmt of its own (a number, a str, a derive): `{}` / `{:?}`
    // as a fmt::Result (#256)) (`.fmt` / `::fmt`, not `.fmt(`: `x.fmt (f)` too)
    if need.contains("fmt") && (sc.has(".fmt") || sc.has("::fmt")) {
        need.extend(["__exr_display_fmt", "__exr_debug_fmt"]);
    }
    // (ManuallyDrop is used through Deref: `m.0`, `*m`)
    // (`io::Error` / `io::Result<T>` are the prelude's IoError, whose name does not appear)
    // (one scan of the source when `io::` is not written; the two below only after it)
    if sc.has("io::") && (sc.has("io::Error") || sc.has("io::Result")) {
        need.extend(["IoError", "Result"]);
    }
    // (`thread::Result<T>` is a join's result: the prelude's __ThreadPanic is its Err)
    if sc.has("thread::Result") {
        need.extend(["__ThreadPanic", "Result", "Debug"]);
    }
    // (a FusedIterator impl comes with an Iterator impl: the source is scanned only then)
    if (need.contains("Iterator") || need.contains("next")) && sc.has("FusedIterator") {
        need.insert("__fused_mark");
    }
    if sc.has("ManuallyDrop") || sc.has("AssertUnwindSafe") || sc.has("LazyCell") {
        need.extend(["Deref", "deref", "deref_mut"]);
    }
    if sc.has("Pin") {
        need.extend(["Deref", "deref", "deref_mut"]);
    }
    // (the one-thread mpsc channel is a shared queue; one scan: a program using the channel imports or names
    // `mpsc`)
    if sc.has("mpsc") {
        need.extend(["__core_channel", "__core_sync_channel", "__Sndr", "__SyncSndr", "__Rcvr__", "__RecvIter", "Rc", "RefCell", "VecDeque", "borrow_mut", "push_back", "pop_front", "clone", "Option", "Result", "Iterator", "next", "Deref", "deref", "new"]);
    }
    // Standard input and output
    if sc.has("stdin") {
        need.extend(["Stdin", "StdinLock", "StdinLines", "__core_stdin", "IoError", "Result", "Option", "next"]);
    }
    if sc.has("stdout") || need.contains("BufWriter") {
        need.extend(["Stdout", "StdoutLock", "__core_stdout", "IoError", "Result", "write_str", "flush", "as_str"]);
    }
    // (`i32::try_from(x)` is `x.try_into()`)
    if sc.has("try_into") || sc.has("try_from") {
        need.extend(["TryFromIntError", "TryFromSliceError", "Result", "try_into"]);
    }
    if sc.any_pre("open_range", &["..)"], &open_range) {
        need.insert("RangeFrom");
    }
    // `{:e}` / `{:.3E}`: exponent notation
    if sc.any_pre("fmt_uses_exp", &["e}", "E}"], &fmt_uses_exp) {
        need.extend(["__exr_fmt_exp", "__exr_fmt_exp_int", "__exr_udiv128", "__exr_fmt_float"]);
    }
    // `x.into()` calls the expected type's from
    if sc.has(".into()") {
        need.insert("from");
    }
    // (it divides a u128, so the 128-bit division with it, also for a u32 receiver)
    if sc.has("ilog10") {
        need.extend(["__exr_ilog10", "__exr_udiv128"]);
    }
    if sc.has("isqrt") {
        need.insert("__exr_isqrt");
    }
    // 128-bit integers: division and formatting (and `x.isqrt()` on one). Also in the loop below, for a 128-bit type
    // that another prelude item brings in (`NonZeroU128::get`, `impl FromStr for u128`) rather than the program's text
    let isqrt = sc.has("isqrt");
    let wide = |need: &mut crate::fxhash::FxSet<&'static str>| {
        need.extend(["__exr_udiv128", "__exr_sdiv128", "__exr_fmt_int128", "__exr_u128_to_f", "__exr_f_to_u128"]);
        if isqrt {
            need.insert("__exr_isqrt128");
        }
    };
    let mut wide_ops = need.contains("i128") || need.contains("u128");
    if wide_ops {
        wide(&mut need);
    }
    // Floats: formatting, powi and the libm declarations (with f64 / f32 named, or a literal such as `1.5`)
    // (not a function name such as `fn f64(..)`: only a type position, such as `: f64` / `as f64` / `f64::`)
    let float_named = (need.contains("f64") || need.contains("f32")) && sc.any_pre("float_named", &["f64", "f32"], &|user_src: &str| {
        let b = user_src.as_bytes();
        ["f64", "f32"].iter().any(|w| {
            user_src.match_indices(w).any(|(i, _)| {
                let after = b.get(i + 3).copied().unwrap_or(b' ');
                // (`3f32`: the suffix of a number literal counts; `x1f32` / `0x1f32` do not)
                // (`1e3f32` / `1e-3f32`: the exponent is part of the number; `x1e-3f32` is `x1e - 3f32`, where the
                // digits alone are the literal)
                let starts_lit = |k: usize| k < i && b[k].is_ascii_digit() && !(k > 0 && (b[k - 1].is_ascii_alphanumeric() || b[k - 1] == b'_'));
                let mut k0 = i;
                while k0 > 0 && (b[k0 - 1].is_ascii_digit() || b[k0 - 1] == b'_') {
                    k0 -= 1;
                }
                let mut k = i;
                loop {
                    while k > 0 && (b[k - 1].is_ascii_digit() || b[k - 1] == b'_') {
                        k -= 1;
                    }
                    if k >= 2 && matches!(b[k - 1], b'e' | b'E') && b[k - 2].is_ascii_digit() {
                        k -= 1;
                    } else if k >= 3 && matches!(b[k - 1], b'+' | b'-') && matches!(b[k - 2], b'e' | b'E') && b[k - 3].is_ascii_digit() {
                        k -= 2;
                    } else {
                        break;
                    }
                }
                let lit_suffix = starts_lit(k) || starts_lit(k0);
                let before_ident = !lit_suffix && i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
                let after_ident = after.is_ascii_alphanumeric() || after == b'_';
                let mut j = i;
                while j > 0 && b[j - 1] == b' ' {
                    j -= 1;
                }
                let after_fn = j >= 2 && &b[j - 2..j] == b"fn";
                !before_ident && !after_ident && after != b'(' && !after_fn
            })
        })
    });
    let float_lit = !float_named && sc.any("float_lit", &|user_src: &str| float_lit_in(user_src));
    // (`d.as_secs_f64()`: the float comes from the prelude's Duration)
    let float_dur = (need.contains("Duration") || need.contains("Instant")) && (sc.has("_f64") || sc.has("_f32"));
    if float_named || float_lit || float_dur {
        need.extend(["__exr_fmt_float", "__exr_powi", "__exr_powif", "__exr_ordering"]);
        // (`x.classify()`: the prelude's classifier, which codegen calls, and its FpCategory)
        if sc.has("classify") {
            need.extend(["__exr_fp_classify", "FpCategory"]);
        }
        // (paths such as `f64::max`: the wrapper whose name ends with the same name)
        for (a, bb) in [("f64", "__exr_f64_"), ("f32", "__exr_f32_")] {
            if sc.has(&format!("{a}::")) {
                for (_, name, _) in items() {
                    if let Some(n) = name.as_deref() {
                        if let Some(m) = n.strip_prefix(bb) {
                            if sc.has(&format!("{a}::{m}")) {
                                need.insert(n);
                            }
                        }
                    }
                }
            }
        }
        for (_, name, _) in items() {
            if let Some(n) = name.as_deref() {
                if n.starts_with("__exr_") && matches!(n.trim_start_matches("__exr_").trim_end_matches('f'),
                    "fmod" | "pow" | "exp" | "exp2" | "log" | "log10" | "log2" | "sin" | "cos" | "tan" | "asin" | "acos"
                    | "atan" | "atan2" | "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" | "cbrt" | "hypot"
                    | "round" | "expm1" | "log1p")
                {
                    need.insert(n);
                }
            }
        }
    }
    // `x.extend(arg)`: codegen makes arg into_iter / iter, and calls __extend_ref when the elements are references
    if need.contains("extend") {
        need.extend(["__extend_ref", "into_iter", "iter", "Iterator"]);
    }
    // (with an adapter chain, the next of its source (Range and so on) is needed too)
    if chain {
        need.insert("next");
    }
    // `Box<dyn Error>` and `impl Error for E {}`, `fn main() -> Result<..>`
    if sc.has("dyn Error") || sc.has("error::Error") || sc.has("Error for") {
        need.extend(["__DynError", "__exr_dyn_error", "__err_mark", "String", "format", "Display", "Debug"]);
        need.extend(FMT_NAMES.iter().copied());
    }
    if sc.has("fn main() -> Result") {
        need.extend(["__exr_main_err", "process_exit", "eprintln", "__exr_eprint", "Debug"]);
        need.extend(FMT_NAMES.iter().copied());
    }
    // (`map[&k]` / `map["k"]` is `map.get(..).unwrap()`)
    if (need.contains("HashMap") || need.contains("BTreeMap")) && sc.has("[") {
        need.extend(["get", "unwrap", "Option"]);
    }
    // (the into_iter of `impl IntoIterator for &Coll` returns an iterator)
    if sc.has("IntoIterator for") {
        need.extend(["__into_iter_mark", "Iterator", "next", "iter"]);
    }
    let iter_lib = need.contains("Iterator")
        || need.contains("HashMap")
        || need.contains("BTreeMap")
        || need.contains("BTreeSet")
        || need.contains("VecDeque")
        || ["split", "split_whitespace", "lines"].iter().any(|n| need.contains(n))
        || need.contains("chars")
        || chain
        || (producer && need.contains("next"))
        // (a slice iterator's type named: `xs: &mut std::slice::Iter<'a, T>`, iterated with no `iter()` of its own)
        || ["Iter", "IterMut"].iter().any(|n| need.contains(n));
    // Ordering: `<` is no identifier, so names that suggest comparison (Ord / sort / cmp and so on) add the glue
    // (an iterator's max / min use `>=` inside too)
    const ORD_NAMES: &[&str] = &[
        "Ord", "PartialOrd", "Ordering", "cmp", "partial_cmp", "sort", "sort_by", "sort_by_key",
        "sort_unstable", "sort_unstable_by", "sort_unstable_by_key", "max_by_key", "min_by_key",
        "max_by", "min_by", "Reverse", "clamp", "binary_search", "binary_search_by",
        "binary_search_by_key", "BinaryHeap", "sort_by_cached_key",
    ];
    // (the prelude's Reverse impls PartialOrd / Ord too)
    let user_impls = user_impl_traits(user_src);
    let cmp_impl = user_impls.iter().any(|t| matches!(*t, "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Clone" | "Copy" | "Default"))
        || need.contains("Reverse");
    let mut op_impls: Vec<&str> = user_impls.iter().copied().filter(|t| OP_TRAITS.contains(t)).collect();
    // (the prelude's PathBuf impls Deref and From)
    if need.contains("PathBuf") {
        op_impls.extend(["Deref", "From", "IntoIterator"]);
    }
    // (std::net's addresses impl From: `Ipv4Addr::from([a, b, c, d])`, `IpAddr::from(v4)`)
    if ["Ipv4Addr", "Ipv6Addr", "IpAddr"].iter().any(|n| need.contains(*n)) {
        op_impls.push("From");
    }
    if need.contains("Cow") || need.contains("Mutex") || need.contains("RwLock") {
        op_impls.push("Deref");
    }
    // (the prelude's TryLockError impls From<PoisonError<T>>)
    if need.contains("TryLockError") {
        op_impls.push("From");
    }
    // (the prelude's Cow impls From: `"x".into()` as a `Cow<str>`)
    if need.contains("Cow") {
        op_impls.push("From");
    }
    if need.contains("Mutex") {
        need.extend(["MutexGuard", "Deref", "deref", "Result"]);
    }
    if need.contains("RwLock") {
        need.extend(["RwLockReadGuard", "RwLockWriteGuard", "Deref", "deref", "deref_mut", "Drop", "drop", "Result"]);
    }
    // (BinaryHeap's peek_mut returns a guard too)
    if need.contains("BinaryHeap") && need.contains("peek_mut") {
        need.extend(["PeekMut", "deref", "deref_mut", "Drop", "drop"]);
    }
    // (RefCell's borrow / borrow_mut return guards: their types and deref / drop)
    if need.contains("RefCell") {
        need.extend(["Ref", "RefMut", "deref", "deref_mut", "Drop", "drop"]);
    }
    if op_impls.contains(&"IntoIterator") {
        need.extend(["__into_iter_mark", "Iterator", "next", "iter"]);
    }
    // (recursive types go through Box / Vec, so those add the glue; codegen expands other comparisons inline)
    if ["Box", "Vec", "vec", "String"].iter().any(|n| need.contains(n)) {
        need.insert("__exr_cmp");
    }
    if cmp_impl {
        need.extend(["eq", "cmp", "partial_cmp", "__exr_eq_marker", "Ordering", "__exr_copy_marker"]);
    }
    // (trait names are not among the prelude name candidates, so derive(Ord) is found in the source text)
    if ORD_NAMES.iter().any(|n| need.contains(n))
        || (iter_lib && (need.contains("max") || need.contains("min")))
        || (sc.has("Ord)") || sc.has("Ord,") || sc.has("Ord for"))
            && (need.contains("max") || need.contains("min") || need.contains("clamp"))
    {
        need.insert("__exr_cmp");
        // (turns Ordering into i32 when the glue calls a hand-written impl)
        if cmp_impl || need.contains("Ordering") {
            need.extend(["__exr_ord_i32", "__exr_pord_i32"]);
        }
        if sc.has("PartialOrd<") || sc.has("PartialOrd for") {
            need.extend(["__exr_ord_i32", "__exr_pord_code"]);
        }
        if need.contains("Reverse") {
            need.extend(["cmp", "partial_cmp"]);
        }
        if need.contains("cmp") || need.contains("partial_cmp") {
            need.extend(["__exr_cmp_m", "__exr_pcmp_m", "__exr_pcmp_fl"]);
        }
        if need.contains("max") || need.contains("min") {
            need.extend(["__exr_max", "__exr_min"]);
        }
        if need.contains("clamp") {
            need.insert("__exr_clamp");
        }
        // For calls as array / slice methods (Vec's methods come with their names)
        for (m, f) in [
            ("sort", "__exr_slice_sort"),
            ("sort_unstable", "__exr_slice_sort"),
            ("sort_by", "__exr_slice_sort_by"),
            ("sort_unstable_by", "__exr_slice_sort_by"),
            ("sort_by_key", "__exr_slice_sort_by_key"),
            ("sort_unstable_by_key", "__exr_slice_sort_by_key"),
            ("sort_by_cached_key", "__exr_slice_sort_by_cached_key"),

            ("binary_search", "__exr_slice_bsearch"),
            ("binary_search_by", "__exr_slice_bsearch_by"),
            ("binary_search_by_key", "__exr_slice_bsearch_by_key"),
        ] {
            if need.contains(m) {
                need.insert(f);
            }
        }
    }
    // (mutating methods of arrays and slices; Vec has its own)
    for (m, f) in [
        ("reverse", "__exr_slice_reverse"),
        ("fill", "__exr_slice_fill"),
        ("rotate_left", "__exr_slice_rotate_left"),
        ("rotate_right", "__exr_slice_rotate_right"),
    ] {
        if need.contains(m) {
            need.insert(f);
        }
    }
    if iter_lib {
        need.insert("Iterator");
        if need.contains("iter") || need.contains("into_iter") {
            need.insert("__exr_slice_iter");
        }
        if need.contains("iter_mut") {
            need.insert("__exr_slice_iter_mut");
        }
        if need.contains("chars") {
            need.insert("__exr_str_chars");
        }
    }
    let mut chosen = vec![false; all.len()];
    // (cross-type comparisons of arrays / slices and Vec: `v == [1, 2]`)
    if !sc.has("no_std")
        && sc.has("[")
        && (sc.has("==") || sc.has("!=") || sc.has("assert_eq!") || sc.has("assert_ne!"))
    {
        need.insert("__exr_seq_eq");
    }
    // (a formatted panic only for programs that use formatting, as it pulls in String and Vec)
    if !sc.has("no_std") && need.contains("__exr_formatter") {
        need.insert("__exr_panic_fmt");
    }
    let mut dur_ops = false;
    let mut fmt_write = false;
    let mut any_trait = false;
    // (the fmt traits of `{:b}` / `{:x}` / `{:e}` .. that the program names: the helper codegen calls for each)
    let mut fmt_radix = 0u8;
    let mut as_mut_gate = false;
    // (whether the program writes `thread` / `Layout`: asked once one of the functions below is wanted)
    let mut thread_text: Option<bool> = None;
    let mut layout_text: Option<bool> = None;
    loop {
        let mut changed = false;
        // (`s.as_mut()` of a String: acg rewrites it to `s.as_mut_str()`. In the loop: String can enter `need` after
        // the rules above, from `fs::` / `env::` / a dyn Error)
        if !as_mut_gate && need.contains("String") && need.contains("as_mut") {
            as_mut_gate = true;
            changed = true;
            need.extend(["as_mut_str", "as_mut_slice"]);
        }
        if !wide_ops && (need.contains("i128") || need.contains("u128")) {
            wide_ops = true;
            changed = true;
            wide(&mut need);
        }
        // (std::fmt::Write, `__FWr`: its impls (`impl __FWr for String`) are chosen by their member's name, and a
        // program that names the trait only in a bound, or calls `write!(w, ..)` on a generic `W`, never names
        // `write_str`. Every impl has it and the trait's defaults call it, so it comes with the trait (#148). In the
        // loop, as the trait can come in through another prelude item too)
        // (`dyn Any` (a panic's payload): the trait, whose one member is type_id, which the name alone would not bring)
        if !any_trait && need.contains("Any") {
            any_trait = true;
            changed = true;
            need.extend(["type_id", "TypeId"]);
        }
        // (std's Pointer: `{:p}` of a type of the program's, and `fmt::Pointer::fmt(&r, f)` of a reference)
        if fmt_radix & 0x40 == 0 && need.contains("Pointer") {
            fmt_radix |= 0x40;
            changed = true;
            need.extend(["__exr_fmt_pointer", "__exr_pointer_fmt", "__exr_fat_pointer_fmt", "__exr_dyn_pointer_fmt", "__exr_fat_words", "__exr_fmt_ptr", "__exr_fmt_fat_ptr", "__exr_fmt_dyn_ptr"]);
        }
        if fmt_radix & 0x3f != 0x3f {
            const RADIX: [(&str, &str); 6] = [
                ("Binary", "__exr_fmt_binary"),
                ("Octal", "__exr_fmt_octal"),
                ("LowerHex", "__exr_fmt_lowerhex"),
                ("UpperHex", "__exr_fmt_upperhex"),
                ("LowerExp", "__exr_fmt_lowerexp"),
                ("UpperExp", "__exr_fmt_upperexp"),
            ];
            for (k, (t, h)) in RADIX.into_iter().enumerate() {
                if fmt_radix & (1 << k) == 0 && need.contains(t) {
                    fmt_radix |= 1 << k;
                    changed = true;
                    need.insert(h);
                }
            }
        }
        if !fmt_write && need.contains("__FWr") {
            fmt_write = true;
            changed = true;
            need.insert("write_str");
        }
        // (the prelude's Duration impls + - * / and += -=; Duration is also chosen as the argument of `thread::sleep`,
        // so this is checked inside the loop)
        if !dur_ops && need.contains("Duration") {
            dur_ops = true;
            op_impls.extend(["Add", "Sub", "Mul", "Div", "AddAssign", "SubAssign"]);
            need.extend(["add", "sub", "mul", "div", "add_assign", "sub_assign"]);
        }
        for (i, (it, name, core_only)) in all.iter().enumerate() {
            if chosen[i] || matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader) {
                continue;
            }
            // (most items are wanted only by their name: one lookup decides them before the gates below)
            if !by_more_than_name(i) && !name.as_deref().is_some_and(|n| need.contains(n)) {
                continue;
            }
            // (std's functions of common names: `thread::current()` / `thread::park()` only with `thread` written,
            // `alloc::alloc(layout)` and the rest, which take a Layout, only with `Layout` written. Here, as the name can
            // come from another prelude item too: HashMap's own `alloc`)
            if matches!(it.kind, ItemKind::Normal) {
                let ok = match name.as_deref() {
                    Some("current" | "park" | "park_timeout") => *thread_text.get_or_insert_with(|| sc.has("thread")),
                    Some("alloc" | "alloc_zeroed" | "dealloc" | "realloc" | "handle_alloc_error") => *layout_text.get_or_insert_with(|| sc.has("Layout")),
                    _ => true,
                };
                if !ok {
                    continue;
                }
            }
            // Iterator's (default) methods only when Iterator is used (the max of `a.max(b)` does not look them up)
            if let ItemKind::TraitMember(h) = it.kind {
                if !iter_lib && is_iterator_trait(h as usize) {
                    continue;
                }
                // Comparison trait declarations only when the user impls one by hand (derives and bounds need no declaration)
                if !cmp_impl && is_cmp_trait(h as usize) {
                    continue;
                }
                if op_trait_name(h as usize).is_some_and(|n| !op_impls.contains(&n))
                    // (the Error trait for the prelude's own `impl Error for IoError ..`)
                    && !(name.as_deref() == Some("__err_mark") && need.contains("Error"))
                {
                    continue;
                }
                // (declarations of Hash / Hasher and so on only when the trait's name is needed: not by names like `fn write`)
                if let Some(tn) = gated_trait(h as usize) {
                    if !need.contains(tn) {
                        continue;
                    }
                }
            }
            // (impls such as `impl Read for &[u8]` also only when the trait's name is needed)
            if let ItemKind::ImplMember(h) = it.kind {
                if let Some(tn) = gated_impl(h as usize) {
                    if !need.contains(tn) {
                        continue;
                    }
                }
            }
            // The same for the methods of `impl Iterator for X` and the methods that make iterators
            if let ItemKind::ImplMember(h) = it.kind {
                if !iter_lib
                    && (is_iterator_trait(h as usize)
                        || matches!(name.as_deref(), Some("iter" | "iter_mut" | "into_iter" | "chars" | "bytes")))
                {
                    continue;
                }
            }
            // Cross-type array / slice comparisons (like `impl PartialEq<[U]> for [T; N]`) only with comparisons and arrays
            if let ItemKind::ImplMember(h) = it.kind {
                if is_seq_eq_impl(h as usize) {
                    if need.contains("__exr_seq_eq") && owner(h as usize).is_none_or(|o| need.contains(o)) {
                        chosen[i] = true;
                        changed = true;
                        add_idents(&mut need, it);
                        if !chosen[h as usize] {
                            chosen[h as usize] = true;
                            add_idents(&mut need, &all[h as usize].0);
                        }
                    }
                    continue;
                }
            }
            // Vec / String methods only when the type's name is needed (`push` alone does not look them up).
            // The drop of `impl Drop for Vec<T>` is needed whenever Vec is, even though its name does not appear
            let mut drop_member = false;
            if let ItemKind::ImplMember(h) = it.kind {
                if let Some(o) = owner(h as usize) {
                    if !need.contains(o) {
                        continue;
                    }
                    // (eq too, as `==` does not appear as an identifier)
                    drop_member = name.as_deref() == Some("eq") || is_drop_impl(h as usize);
                }
            }
            // core::cmp::max and the like are usable only with a `core` / `std` identifier (in a path or a use)
            // (`drop(x)` is usable as a prelude function)
            if *core_only
                && !(need.contains("core") || need.contains("std"))
                && name.as_deref() != Some("drop")
            {
                continue;
            }
            // (`impl Error for IoError { fn __err_mark .. }`: when the program names Error and that type)
            // (and the trait itself with them)
            let err_impl = name.as_deref() == Some("__err_mark")
                && need.contains("Error")
                && match it.kind {
                    ItemKind::ImplMember(h) => all[h as usize].0.idents.get(3).is_some_and(|&(a, b)| need.contains(&PRELUDE[a as usize..b as usize])),
                    ItemKind::TraitMember(_) => true,
                    _ => false,
                };
            // A variant name (Some and so on) needs its enum
            let wanted = err_impl || drop_member || match name {
                Some(n) => need.contains(n.as_str()),
                None => false,
            } || (name.as_deref() == Some("Option")
                && (need.contains("Some") || need.contains("None")))
                || (name.as_deref() == Some("Result")
                    && (need.contains("Ok") || need.contains("Err")))
                || (name.as_deref() == Some(BOX_ALLOC) && need.contains("Box"))
                // String < (sort / max and so on)
                || (name.as_deref() == Some("__exr_str_cmp")
                    && (need.contains("sort") || need.contains("String") || need.contains("max") || need.contains("min")))
                // A range may be used as a value (`(0..n).map(..)`)
                || (matches!(name.as_deref(), Some("Range") | Some("RangeInclusive"))
                    && need.contains("Iterator"))
                || (matches!(name.as_deref(), Some(BOX_FREE) | Some(DROP_GLUE))
                    && (need.contains("Box") || need.contains("Drop")))
                || (name.as_deref().is_some_and(|n| n.starts_with("__exr_fnptr"))
                    && need.contains("dyn"));
            if wanted {
                chosen[i] = true;
                changed = true;
                add_idents(&mut need, it);
                if let ItemKind::ImplMember(h) | ItemKind::TraitMember(h) = it.kind {
                    if !chosen[h as usize] {
                        chosen[h as usize] = true;
                        let hit = &all[h as usize].0;
                        add_idents(&mut need, hit);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = String::new();
    let mut open_impl = false;
    // (items keep their prelude lines, newline-padded: panic lines never vary, so crates can share machine code)
    let mut cur_line = 1usize;
    for (i, (it, _, _)) in all.iter().enumerate() {
        let in_impl = matches!(it.kind, ItemKind::ImplMember(_) | ItemKind::TraitMember(_));
        if open_impl && !in_impl {
            out.push_str("}\n");
            cur_line += 1;
            open_impl = false;
        }
        // An impl's associated type (`type Item = &T;`) always comes with its impl
        // (and a trait's `type Item;`, whenever the trait is: E0191 needs it for `dyn Iterator`)
        let assoc_member = matches!(it.kind, ItemKind::ImplMember(h) | ItemKind::TraitMember(h) if chosen[h as usize])
            && it
                .idents
                .first()
                .is_some_and(|&(a, b)| &PRELUDE[a as usize..b as usize] == "type");
        if !chosen[i] && !assoc_member {
            continue;
        }
        while cur_line < it.line {
            out.push('\n');
            cur_line += 1;
        }
        let text = &PRELUDE[it.start..it.end];
        out.push_str(text);
        out.push('\n');
        cur_line += text.bytes().filter(|&c| c == b'\n').count() + 1;
        if matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader) {
            open_impl = true;
        }
    }
    if open_impl {
        out.push_str("}\n");
    }

    out
}

#[cfg(test)]
mod cstr_literal_tests {
    /// A C string literal starts a token: `"c"` and `abc"` are ordinary strings (examples/shared.rs took the C string
    /// helpers for `Node::new("c", 4)`)
    #[test]
    fn c_string_literal_starts_a_token() {
        assert!(super::has_cstr_literal("let s = c\"hi\";"));
        assert!(super::has_cstr_literal("f(cr\"raw\")"));
        assert!(super::has_cstr_literal("c\"at the start\""));
        assert!(!super::has_cstr_literal("Node::new(\"c\", 4)"));
        assert!(!super::has_cstr_literal("let abc\" = 1;"));
        assert!(!super::has_cstr_literal("let spec\"x\" = 1;"));
    }
}
