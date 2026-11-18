# Killing the missed mutants in ide (2026-11-17)

Date: 2026-11-17

Ran all 3,142 mutants of ide (crates/ide) with `-j 3` and cut it off after 2 hours (at 1,897 mutants). Results: 1,165 caught,
668 missed, 44 timeouts, 20 unviable. All 668 missed mutants are in navigate.rs. The most common are
local bindings (`live` 79, `arm_binding` 48, `local_type` 48, `pat_binding` 20, `fn_param` 19), `def_at` 60,
the use tree (`use_tree` 27, `resolve_use` 18, `reexport` 18), and small string helpers (`targs`, `generic_names`, `captures`,
`cfg_pred`, `macro_bodies`, etc.). The golden tests (fixtures) only exercised common code patterns and did not check boundaries or one side of branches.

- Added tests: `crates/ide/src/navigate_tests.rs` (made a child module of navigate with `#[path]`, so it calls private helpers directly).
  - String helpers: `captures` / `generic_names` / `targs` / `subst` / `alias_parts` / `ret_type` / `elem_type` /
    `index_type` / `type_head` / `name_hash` (known FNV-1a values) / `mod_score` / `use_module` / `lib_rank`.
  - cfg: `any` / `all` / `not` / `target_os` / `target_family` / `unix` / `windows` / `miri` / unknown predicates.
  - The range of `macro_rules!` bodies (`{}` / `()` / inside a function / unclosed), the use tree (nested, `as`, glob, `self`).
  - Locals: let / for / if let / while let / match arms and guards / or patterns / closures / parameter patterns /
    the lifetime of bindings inside blocks. The binding checks (`pat_binding` / `fn_param` / `arm_binding`) in both positive and negative cases.
  - Types: annotations (`&'static P`, `&mut a::b::P`), `T(..)`, `T { .. }`, `U::new()`, `a::b::P`.
  - 16 derive methods (`clone` / `default` / `eq` / `ne` / `fmt` / `partial_cmp` / `lt` .. / `hash`).
  - Definitions and references of format captures `{x}` / `{y:?}` / `{{x}}`, and type parameters (fn / struct / impl / trait / enum / type / union).
- Bugs found by the tests (fixed):
  - g in the match guard `S { f: g } if g > 0 =>` was not resolved (the binding was introduced at `=>`). It now looks ahead to see whether
    `if` is a guard (`guard_arrow`), and makes the binding visible from the guard.
  - The type of v in `let v = a::b::P;` became `b` (the module name was used as the type). Same for `a::b::T(..)` / `m::f()`.
  - The type of the annotation `&'static P` became `static` (lexically, a lifetime is the identifier after `'`).
  - In the or pattern `(m, 0) | (0, m)`, m resolved to the later occurrence. It now resolves to the earlier one, as in rust-analyzer.
  - In `apply_cfg`, when recounting the derives of items excluded by cfg, the excluded ranges had not been added yet.
  - The LSP looked for `#[path = "x.rs"] mod m;` directly under a file at `<file name>/x.rs` (rustc looks from the directory containing
    the file). Because of this, navigate_tests.rs was not read, and zustc's references match appeared to drop from 78% → 58%.
- Speed: `python3 bench/lsp_parity.py` (on the zustc target) gives defi 1.0 / hove 0.9 / comp 1.2 / refe 15.1 ms, the same as before
  (rust-analyzer is 0.9 / 0.9 / 5.6 / 27.3 ms). The match rates are also the same as before (definition 100%, hover 82%, references 78%).
  The remaining references differences are things rust-analyzer counts inside cfg-excluded items, in generated files in OUT_DIR, and in
  lexer.rs, which is shared with build.rs.

After adding the tests, the run was redone on just the 967 mutants of the helper functions above (37 functions) (`-F` for the function names, `-j 3`, 82 minutes.
For the ones still missed, more tests were added and the run was repeated with `--iterate`):

| | Caught | Missed | Timeouts |
|---|---:|---:|---:|
| Before adding (the same functions, from the partial full run) | — | 441 | — |
| Run 1 (967 mutants) | 715 | 213 | 39 |
| Run 2 (the remaining 249 with `--iterate`) | 749 | 179 | 39 |

- Timeouts are mutants such as `+=` → `-=` that never finish (the test hangs, so this is the same as caught).
- Most of the remaining 179 are mutants that do not change the result:
  - Boundary `<` → `<=` (`sym` / `ident` return false outside the range, so the result is the same).
  - Early exits (the first exclusion in `arm_binding` / `pat_binding`, the cutoff at `{` / `;` in `guard_arrow`): the later scan or
    the range check in `arm_pat` gives the same answer. They exist for speed.
  - The scan for `>` in `generic_names`: `targs` splits the result again, so it is the same. It exists to avoid copying the rest of the file.
