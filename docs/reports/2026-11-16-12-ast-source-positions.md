# Source positions in our own syntax tree

Date: 2026-11-16

- Added `parse::Span { start, at }` to the expressions used for panic locations (Unary / Binary / AssignOp / MethodCall / Index / Macro).
  start is the start of the expression (syn's `e.span()`); at is the per-expression position (the method name, `[`, the operator, the macro's opening bracket)
- The text of expression macro arguments is built with the original line and column layout (`group_text_laid_out`). When re-parsing, it is offset from the macro's position
  (`parse_expr_at` / `ast_closure::macro_args_at`), so things like `unwrap()` inside `println!` arguments also get their original position
- acg's `loc()` uses this position: the locations of out-of-range indexing (`[` for Vec, the start of the expression for arrays), overflow, division by zero, `unwrap` / `expect` (the method name), and
  `panic!` / `assert!` / `dbg!` (the start of the macro) now match rustc. `tests/panic_loc.rs` compares with rustc
- `tests/driver2.rs` now also compares stderr (panic messages) (all 139 match). For `matches!` arguments, type inference and code generation use the same syntax tree (`ast_closure::matches_parts`)
- For the column of `dbg!`, the syn version printed the position of the argument (rustc uses the macro's position). The syn version was fixed too
- ast_codegen.rs (v1), made at the start of the port, was replaced by acg and deleted (tests/ast_codegen.rs runs tests/acg through driver2)
- Compile errors also get locations: Path / Field / Lit also carry positions, and acg errors get the `line:col:` of "the innermost expression that has a position"
  (the name position for method calls and fields, the initializer for a let type mismatch, the argument itself for an argument type mismatch,
  the value for return and tail expressions). For syntax errors and item summary errors, line numbers within the item are converted to file line numbers.
  `tests/compile_errors.rs` checks on tests/compile_errors/*.rs that the messages (including locations) are the same as with the syn-version driver
