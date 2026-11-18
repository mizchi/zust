# Self-hosting stage 8: moving closure analysis to the own syntax tree

Date: 2026-11-15

- `src/ast_closure.rs`: performs the same analysis as closure.rs (on syn's syntax tree) (closures in a body in pre-order, including inside macro arguments; free variables;
  whether a capture is used as a value) on the syntax tree of src/parse.rs. `tests/ast_closure.rs` checks that it matches the syn version on all 1273 closures across all files
- Fix: a closure mixing variables passed as values and variables that are assigned (`|| { r = cnt(caps); }`) captured everything by value, so the outer `r` did not change
  (if any capture is assigned, it is held by reference)
- Fix: passing `&Vec<T>` to a `&[T]` argument made type inference unify the two references as the same type, so a `&Vec<T>` captured by a closure was treated as `&[T]`
- Fix: variables inside the guard of `matches!(x, p if guard)` were not counted as captures
- Passing `&Box<T>` / `&&T` to a `&dyn FnMut(&T)` argument (deref coercion)
