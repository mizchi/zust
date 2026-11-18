# Self-hosting stage 3: prelude.rs

Date: 2026-11-15

- `include_str!("..")` / `include_bytes!("..")` (paths relative to the input file; the contents are escaped into a one-line literal)
- `std::sync::OnceLock` (`get_or_init` / `get` / `set`). Non-mut statics are also placed in writable memory (interior mutability)
- `str::rsplit` / `rsplit_once` / `match_indices`, `starts_with` / `ends_with` with closure patterns, `Iterator::find_map`
- `contains` on slices whose elements are not integers etc. (`NAMES.contains(&w)`, compared with PartialEq)
- `flatten` over a sequence of references (yielding `&T` in order from a `&Vec<T>` from `map.get(k)` etc.)
- const patterns (`Some(BOX_FREE) | Some(DROP_GLUE)`, consts whose initial value is a literal)
- Type inference for `&s[a..b]` (s: &str / String), method chains inside `matches!` (the type of a re-parsed expression is looked up by position)
- Fix: the drop flag of a temporary on the right side of `a || f(&format!(..))` was not initialized when the right side was not evaluated (it is now set to 0 at the entry)
