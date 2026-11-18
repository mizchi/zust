# Self-hosting stage 19: compiling zustc (the path without syn and Cranelift) with zustc

Date: 2026-11-16

- `tests/selfhost/build_compiler.py`: bundles driver2 + acg + minclif and every module they use (lexer / macros / prelude / types / parse /
  ast_* / program / solver / ctors / ast_infer / ast_consteval / link) into a single-file compiler (about 31 thousand lines)
  (`compiler <in.rs> -o <out> [-O]`)
- When compiled with zustc (driver2), it can actually compile and run programs (zustc compiled itself)
- `tests/selfhost_compiler.sh`: compiles probe / examples / acg with the two compilers, the rustc version and the zustc version, and compares the .o files and run results.
  It then compiles itself once more with the zustc version (second stage)
- Result: **for all 139 programs, the rustc-built and zustc-built compilers emit byte-identical .o files, and the run results are also the same**.
  Recompiling itself with the zustc-built compiler gives the same .o (about 18 MB) as the first stage (fixed point)
- Found by comparison: in `matches!(tmp, Some(V(x)) if ..)` the binding moved out of a temporary and it was dropped twice (also present in the syn version),
  and the overflow checks for i128 `abs` / add/sub in acg
- Fixed along the way: names when flattening modules (definitions with the same name as `std::path::Path`, uses visible through `use super::*`, `&'static`),
  type inference (`len` with an unknown receiver type, annotated lets, `x.clone()` on integers), deref coercions in acg (`&&T` → `&T`, `&String` → `&str`),
  auto-reference of the receiver for range indexing, closures inside a `matches!` guard, zero extension in minclif's bitcast. Constructs zustc cannot handle yet
  (destructuring assignment, `collect::<Option<Vec<_>>>`, conversion from closures to fn pointers, the values of `std::f64::consts`, etc.) were rewritten in the source
