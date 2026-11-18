# Self-hosting stage 6: macros.rs

Date: 2026-11-15

- Variants with the same name as a struct (`TokenTree::Punct(Punct { .. })`: a bare name prefers the struct)
- `&a == &b` (a, b references to structs etc.), both `impl PartialEq` and `impl PartialEq<str>`
- `Option::flatten`, HashMap `clone` (before, it was a shallow copy and caused a double free), `String::replace_range` / `into_bytes`
- Fix: uppercase names in patterns, such as `None`, were counted as closure capture candidates
- Fix: `include_str!` expansion also expanded occurrences inside the second and later comments and strings on the same line
