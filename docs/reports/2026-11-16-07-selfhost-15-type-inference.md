# Self-hosting stage 15: moving type inference to our own syntax tree

Date: 2026-11-16

- `src/ast_infer.rs`: ported the syntax-tree-walking part of infer.rs (patterns, blocks, expressions, method calls, closures, format macro arguments, etc., 1,900 lines) to
  our own syntax tree. It shares the unifier in solver.rs. `tests/ast_infer.rs` checks that, on probe / examples and zustc's own modules
  (`out/selfhost/program_main.rs`), the 885 functions and 2,380 lets get the same types as the syn version
- Inference results are recorded by source position in addition to node address, so nodes that codegen duplicates while rewriting (such as the expression inside `__exr_slice_to_vec(&a[..])`) can also look up their types
- Added: the `DerefMut` trait, fields through Deref (`self.prog` with self: `&mut Inf`), `Option::iter` / `into_iter` and `it.chain(Some(x))`,
  `split` with a closure (`s.split(|c| ..)`)
- Fixes:
  - `let Some(name) = name else { .. }` (shadowing the outer variable with the same name) did not stop the drop of the outer variable, so it was freed twice
  - moving a field out of a temporary (`f().0`) dropped the whole temporary (now only the remaining fields are dropped)
  - mutually recursive types (`Ty` ↔ `FnTy`) got the same deep hash, so cache keys of instances for different types (`HashMap::find<&FnTy>` and `<&Ty>`) collided
  - our own parser read `&&T` as a triple reference
  - the `const` in `*const T` was taken as a definition in a module namespace
- Workarounds (for things zustc does not support yet): trait methods not in scope are not removed from the candidates (`Solver::deref` was renamed to `deref_var`),
  `std::mem::discriminant` (replaced with a function returning the shape number)
