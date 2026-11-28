//! The frontend flow (shared by the compiler and LSP / analysis): preprocessing (prep) → splitting into items (split, incremental) →
//! appending the prelude items that may be used. Given what is remembered across edits (FrontMemo), unchanged parts are not rebuilt

use crate::lexer::{self, ItemKind, SrcItem};
use crate::prof::{Timer, now_ns};

pub type R<T> = Result<T, String>;

/// Appends the prelude items that may be used after the user's source (same as driver::with_prelude).
/// Returns (the whole source, the items, the number of user items)
pub fn with_prelude(src: &str) -> R<(String, Vec<SrcItem>, usize)> {
    with_prelude_in(src, &[], &mut vec![], None, &mut Default::default()).map(|(s, i, u, _)| (s, i, u))
}

/// What preprocessing remembers across edits (so that LSP's didChange does not rebuild unchanged parts)
#[derive(Default)]
pub struct FrontMemo {
    pub sel: crate::prelude::SelectCache,
    /// The input and result of the previous split
    pub split: Option<(String, Vec<SrcItem>)>,
    /// The result per preprocessing chunk
    pub prep: crate::prep::PrepMemo,
}

fn clone_item(it: &SrcItem) -> SrcItem {
    SrcItem { start: it.start, end: it.end, line: it.line, hash: it.hash, idents: it.idents.clone(), kind: it.kind }
}

fn same_items(a: &[SrcItem], b: &[SrcItem]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.start == b.start && a.end == b.end && a.line == b.line && a.hash == b.hash && a.idents == b.idents && a.kind == b.kind)
}

fn count_nl(b: &[u8]) -> usize {
    b.iter().filter(|&&c| c == b'\n').count()
}

/// Builds lexer::split incrementally from the previous input and result: the items of the unchanged parts before and after are used as they are (those after with their positions shifted),
/// and only the middle is split again. The boundaries are the starts of top-level items beginning after whitespace (impl / trait blocks as a whole).
/// When the end of the middle may be inside a comment or a string, or when split fails, the whole is split
#[doc(hidden)]
pub fn split_inc(src: &str, memo: &mut Option<(String, Vec<SrcItem>)>) -> R<Vec<SrcItem>> {
    let r = match memo.as_ref() {
        Some((old, items)) if std::env::var_os("ZUSTC_NO_SPLIT_INC").is_none() => split_diff(old, items, src),
        _ => None,
    };
    let items = match r {
        Some(v) => v,
        None => lexer::split(src)?,
    };
    if std::env::var_os("ZUSTC_SPLIT_VERIFY").is_some() {
        if !same_items(&lexer::split(src)?, &items) {
            return Err("internal: incremental split differs from the full split".into());
        }
    }
    *memo = Some((src.to_string(), items.iter().map(clone_item).collect()));
    Ok(items)
}

fn split_diff(old: &str, items: &[SrcItem], new: &str) -> Option<Vec<SrcItem>> {
    let (p, n) = (old.as_bytes(), new.as_bytes());
    if p == n {
        return Some(items.iter().map(clone_item).collect());
    }
    let lim = p.len().min(n.len());
    let mut pre = 0;
    while pre < lim && p[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < lim - pre && p[p.len() - 1 - suf] == n[n.len() - 1 - suf] {
        suf += 1;
    }
    // Boundaries: (item index, position)
    let mut groups: Vec<(usize, usize)> = vec![];
    for (k, it) in items.iter().enumerate() {
        let top = !matches!(it.kind, ItemKind::ImplMember(_) | ItemKind::TraitMember(_));
        if top && (it.start == 0 || p[it.start - 1].is_ascii_whitespace()) {
            groups.push((k, it.start));
        }
    }
    // Before: those whose boundary range (up to the next boundary) lies before the changed part
    let mut j = 0;
    while j + 1 < groups.len() && groups[j + 1].1 <= pre {
        j += 1;
    }
    // (groups[j] contains the changed part; if what precedes the first item changed, from the beginning)
    let (keep_n, a) = if groups.is_empty() || groups[j].1 > pre { (0, 0) } else { (groups[j].0, groups[j].1) };
    // After: the first boundary starting after the changed part
    let chg_end = p.len() - suf;
    let m = groups.iter().position(|g| g.1 > chg_end && g.1 > a);
    let (tail_k, b_old) = match m {
        Some(m) => (groups[m].0, groups[m].1),
        None => (items.len(), p.len()),
    };
    let b_new = (b_old + n.len()).checked_sub(p.len())?;
    if b_new < a {
        return None;
    }
    let mid = &new[a..b_new];
    let mut mids = lexer::split(mid).ok()?;
    // (after the items at the end of the middle: if there is anything that may have left a comment or string open, do not use it)
    let last_end = mids.last().map(|x| x.end).unwrap_or(0);
    let gap = &mid.as_bytes()[last_end..];
    if gap.windows(2).any(|w| w == b"/*") || gap.iter().any(|&c| c == b'"' || c == b'\'' || c == b'#') {
        return None;
    }
    let mut out: Vec<SrcItem> = Vec::with_capacity(keep_n + mids.len() + items.len() - tail_k);
    out.extend(items[..keep_n].iter().map(clone_item));
    let lines_a = count_nl(&n[..a]);
    for it in &mut mids {
        it.start += a;
        it.end += a;
        it.line += lines_a;
        for x in &mut it.idents {
            x.0 += a as u32;
            x.1 += a as u32;
        }
        it.kind = match it.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember(h + keep_n as u32),
            ItemKind::TraitMember(h) => ItemKind::TraitMember(h + keep_n as u32),
            k => k,
        };
    }
    out.extend(mids);
    let dk = out.len() as i64 - tail_k as i64;
    let dpos = n.len() as i64 - p.len() as i64;
    let dline = count_nl(&n[..b_new]) as i64 - count_nl(&p[..b_old]) as i64;
    for it in &items[tail_k..] {
        let mut x = clone_item(it);
        x.start = (x.start as i64 + dpos) as usize;
        x.end = (x.end as i64 + dpos) as usize;
        x.line = (x.line as i64 + dline) as usize;
        for y in &mut x.idents {
            y.0 = (y.0 as i64 + dpos) as u32;
            y.1 = (y.1 as i64 + dpos) as u32;
        }
        x.kind = match x.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember((h as i64 + dk) as u32),
            ItemKind::TraitMember(h) => ItemKind::TraitMember((h as i64 + dk) as u32),
            k => k,
        };
        out.push(x);
    }
    Some(out)
}

/// crates: the names of the crate modules in the source (lexer::rename_module_collisions_in)
/// fix: the places whose columns shifted from renaming modules (lexer::rename_module_collisions_fix)
/// cfg: the settings for evaluating `#[cfg(..)]` in the macro expansions
/// The fourth return value is the first line of the prelude (line in the prelude = line of the joined source - this)
pub fn with_prelude_in(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>, cfg: Option<&crate::cfg::CfgSet>, cache: &mut FrontMemo) -> R<(String, Vec<SrcItem>, usize, usize)> {
    let mut tm = Timer { on: std::env::var_os("ZUSTC_PROF").is_some(), last: now_ns(), start: now_ns() };
    let renamed = crate::prep::std_renames(src);
    let src: &str = renamed.as_deref().unwrap_or(src);
    let pre = crate::prep::prep(src, crates, fix, cfg, &mut cache.prep)?;
    tm.lap("prep", String::new());
    let mut items = split_inc(&pre, &mut cache.split)?;
    // (the program's types named as the prelude's: renamed, and split again)
    let pre = match crate::prelude::rename_user_collisions(&pre, &items) {
        Some(r) => {
            items = split_inc(&r, &mut cache.split)?;
            r
        }
        None => pre,
    };
    // (the prelude's types the program defines for itself, renamed in the prelude below; the program's std paths to
    // one name the prelude's: rewritten, and split again (rare: a type of the program's named as one, and a std path))
    let defined = crate::prelude::shadowed_defs(&pre);
    let to_shadowed = crate::prelude::std_paths_to_shadowed(&pre, &defined);
    if let Some(r) = &to_shadowed {
        items = split_inc(r, &mut cache.split)?;
    }
    let src: &str = to_shadowed.as_deref().unwrap_or(&pre);
    let user = items.len();
    tm.lap("split", String::new());
    cache.sel.shadow_paths = to_shadowed.is_some();
    let extra = crate::prelude::select_with(src, &items, &mut cache.sel);
    // (the selection's items, unless the text is renamed below: what splitting it would give)
    let picked = cache.sel.items.take().filter(|_| defined.is_empty());
    let extra = crate::prelude::rename_shadowed(&defined, extra);
    tm.lap("prelude", String::new());
    if extra.is_empty() {
        return Ok((src.to_string(), items, user, 0));
    }
    let base = src.len() + 1;
    let lines = src.bytes().filter(|&c| c == b'\n').count() + 1;
    let n0 = items.len() as u32;
    let extra_items = match picked {
        Some(v) => {
            if std::env::var_os("ZUSTC_SPLIT_VERIFY").is_some() && !same_items(&v, &lexer::split(&extra)?) {
                return Err("internal: the prelude selection's items differ from splitting its text".into());
            }
            v
        }
        None => lexer::split(&extra)?,
    };
    for mut it in extra_items {
        it.start += base;
        it.end += base;
        it.line += lines;
        for r in &mut it.idents {
            r.0 += base as u32;
            r.1 += base as u32;
        }
        it.kind = match it.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember(h + n0),
            ItemKind::TraitMember(h) => ItemKind::TraitMember(h + n0),
            k => k,
        };
        items.push(it);
    }
    let mut full = String::with_capacity(base + extra.len());
    full.push_str(src);
    full.push('\n');
    full.push_str(&extra);
    Ok((full, items, user, lines))
}

#[cfg(test)]
mod tests {
    /// The items the selection gives with its text are what splitting the text gives (with_prelude_in appends them
    /// without splitting it), for the programs of tests/acg
    #[test]
    fn selection_items_are_its_split() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/acg");
        let mut checked = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            let items = match crate::lexer::split(&src) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut sc = crate::prelude::SelectCache::default();
            let extra = crate::prelude::select_with(&src, &items, &mut sc);
            if let Some(v) = sc.items {
                assert!(super::same_items(&v, &crate::lexer::split(&extra).unwrap()), "{}", p.display());
                checked += 1;
            }
        }
        assert!(checked > 20, "only {checked} programs selected prelude items");
    }
}
