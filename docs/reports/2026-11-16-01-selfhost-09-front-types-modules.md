# Self-hosting stage 9: splitting out the type part of front.rs, and module namespaces

Date: 2026-11-16

- `src/types.rs`: split out the parts of front.rs that do not depend on syn (`Ty` / `TySyn` / `ArrLen` / intern) (front re-exports them with `pub use`)
- `src/ast_types.rs`: ported front.rs's `parse_tysyn` (syn type → TySyn) to our own syntax tree. `tests/ast_types.rs` checks that it matches the syn version on the 3914 types across all files
  (the trait in `dyn` is rendered as text with the same spacing as proc_macro2)
- Module namespaces: when separate modules define the same name (`types::Ty` and `parse::Ty`), the definition inside the module is renamed to `m__Ty` before flattening,
  and the references inside, `use crate::m::{Ty as X}` (including aliases), and the paths `m::Ty` / `crate::m::Ty` / `super::Ty` are rewritten (enum variant names and fields are not changed). Debug output shows the original name
- Statics with the same name inside functions (a `static TABLE` per `intern`) are renamed per function before being moved out
- `std::sync::Mutex`, `Box::leak`, `std::ptr::eq`, `Result::as_ref`, `HashSet<&T>::get(&t)` (`&T` to `&&T`), hashing of raw pointers, `&&str == &&str`
- Fix: `==` on a type that recurses through a reference (`enum T { R(&'static T) }`) made the compiler run out of stack (it now compares with a glue function)
- Fix: in type inference, passing `&String` to a `&str` argument unified the two reference types (in a match arm with a guard, `t.len()` read a broken value)
- Fix: in `v == b"crate"` (v is `x.to_vec()`, whose type is decided later), v was given the byte-string type
- Fix: nodes created by rewriting `String`'s `contains` and similar had no type record, so the method was not found
- The return type of a call through a closure variable `tok(k)` is looked up with `peek`
