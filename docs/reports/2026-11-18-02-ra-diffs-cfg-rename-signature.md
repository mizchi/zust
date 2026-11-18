# Closing the remaining differences from rust-analyzer: cfg, rename of trait impls, signatureHelp (2026-11-18)

Date: 2026-11-18

- **Inside items excluded by cfg**: matched the behavior confirmed by querying rust-analyzer on a small workspace.
  - Inside excluded items (functions, impls, structs, enums, consts, types, traits, inline mods, statement cfg), rust-analyzer still
    resolves paths to items visible from the outer scope (`used()` / `P` / `sub::deep` / `crate::P` / names imported with use),
    and answers definition / references / hover for them. It does not answer for locals, fields, methods (`p.get()` / `s.x`), associated items (`P::new`),
    or the name of the excluded item itself. Previously zustc excluded everything that was excluded.
  - `def_at` looks up tokens in excluded regions with `inactive_def_at` and answers only the cases above. references, rename and hover also go through this.
  - `apply_cfg`: unset cfgs used to be treated as "true"; now they are "false", as in rustc
    (`no_global_oom_handling` / `miri` / `doc` / unknown `key = ".."`). Added support for `target_arch` / `target_pointer_width` /
    `target_endian` / `target_env` / `target_vendor` / `target_has_atomic` / `panic`. `test` and
    `debug_assertions` are treated as true, same as rust-analyzer.
- **Renaming a method of a trait impl** (`fn default` in `impl Default for S`): rust-analyzer rewrites 1,273 locations, reaching into the std
  declaration and the sources of dependency crates (an edit that breaks the build as-is). zustc does not match this:
  - Trait in the workspace: re-resolve from the trait declaration, and rewrite the declaration, all impls, and the calls.
  - Trait from a library or external crate: refuse with a reason (because the declaration cannot be changed).
- **signatureHelp**:
  - No answer for `st.extend(..)` (`Vec<String>`) because there were two candidates, std's `impl Extend<T> for Vec` and `impl Extend<&T> for Vec`,
    and neither was chosen. For methods with the same owner and same name in the same file, the first one (the generic one) is now used.
    definition / hover now answer in the same way.
  - Closure call `h(..)` (`let h = |name: &str| -> Option<Key> { .. }`): the signature is `impl Fn(name: &str) -> Option<Key>`
    (if the return type is not written, the type of the body expression). rust-analyzer expands `Key` to `u128`.
  - Attached doc comments as `documentation`, same as rust-analyzer.
- **Expression types for literals**: `"a"` → &str, `'c'` → char, `b"x"` → &[u8], `b'x'` → u8, `1u8` / `2.0f32` / `1_000usize`,
  `true` → bool. Only when the whole expression is a literal (the 1 in `x + 1` is not an expression type). Numbers without a suffix (`0` / `1.5`)
  get their type from usage (rust-analyzer makes acc in `let mut acc = 0; acc = add(acc, x_i64)` an i64), so no type is given.

Results (`python3 bench/lsp_parity.py`, target zustc; synth stays at 100% for everything):

| Feature | Before | After | rust-analyzer |
|---|---:|---:|---:|
| references (count match) | 78% | 92% | 100% |
| documentHighlight | 85% | 100% | 100% |
| rename (edit match) | 82% | 94% | 100% |
| signatureHelp | 89% (answered 85%) | 100% (answered 95%) | 100% (answered 95%) |
| Median time refe / rena / sign (ms) | 14.5 / 1.3 / 1.3 | 14.2 / 1.3 / 1.2 | 25.0 / 0.8 / 0.5 |

fixture (`python3 bench/ide_golden_ra.py`): definition 460 → 462/467, references 584 → 606/629 (93% → 96%),
hover 612 → 614/636.

Remaining differences: rename of `default` (the deliberate refusal above). references for `u8` / `Key` (rust-analyzer also counts inside the
generated files in OUT_DIR, and inside lexer.rs, which is shared with build.rs). hover on `ZUSTC_INLINE_LOG` (inside a string).
