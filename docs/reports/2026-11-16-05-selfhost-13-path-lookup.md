# Self-hosting stage 13: separating path lookup from syn

Date: 2026-11-16

- On top of `PPath` in `program.rs` (segment names and whether type arguments are present), moved the logic that strips modules and looks up structs / enum variants / functions by path.
  The syn version in front.rs (`fn_by_path`, etc.) just converts the syn path to a `PPath` and calls it. Paths in our own syntax tree can also be looked up with `PPath::of`
- Fix: when moving items out of a function, also rename those that collide with a top-level item of the same name (two `fn walk`)
