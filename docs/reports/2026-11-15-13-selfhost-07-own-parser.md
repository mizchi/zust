# Self-hosting stage 7: own parser (groundwork for moving away from syn)

Date: 2026-11-15

- `src/parse.rs`: lexing and parsing of Rust with no external crates (items, types, patterns, expressions, statements; precedence, struct literals in `if` heads,
  `>>` and generics, let chains, labels, closures; macro calls stay as token sequences). It is written in a style that zustc can compile
- `tests/parse.rs`: on every file in probe / examples / src, the count per node kind (22 kinds) matches the count from syn
- `let x;` assigned later (it is set up filled with 0)
- Fix: an `Option<T>` field of a non-generic enum was laid out before the size of T was known, so fields overlapped
  (this happens with recursive syntax trees; after re-taking the instances, the non-generic definitions are re-taken too)
- Fix: a user `struct Path` lost to the prelude's `type Path = str`
