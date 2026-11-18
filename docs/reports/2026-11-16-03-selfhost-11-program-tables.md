# Self-hosting stage 11: separating the Program tables from syn

Date: 2026-11-16

- `src/program.rs`: moved front.rs's `Program` / `FnSig` / `StructDef` / `AdtTable` / `Insts`, together with `build` (item summary → tables), layout,
  instantiation, `implements`, `resolve_with`, and more. Lookups that walk the syn syntax tree (`fn_by_path` / `variant_by_path` / `eval_int`, etc.) stay in
  a separate `impl Program` in front.rs. Const evaluation (array lengths) uses our own parser (`parse::parse_expr`)
- `src/key.rs`: split the cache key's `KeyHasher` out of cache.rs (because cache.rs depends on codegen)
- Added: `thread_local!` (expanded into a lazily initialized `static mut` and a unit struct with `with`), statics of unit structs,
  paths to statics inside modules (`m::S`), renaming of names imported with `use crate::m::*`, `&T` → `*const T`,
  `&mut v[i]` (`Vec<Box<T>>`) → `&mut T`, `*g` / `g[i]` on Deref types such as MutexGuard, `Range::contains`, `next_multiple_of`,
  overflow checks for u128 / i128 multiplication (Cranelift has no `umul_overflow.i128`, so it is built from 64-bit pieces),
  `Hash::hash` for tuples and Option (written in the same order as std, so DefaultHasher values also match), `it.chain([(&a, &b)])`,
  `&&u32 == &&u32`, type () for the `print!` family
- Fixes:
  - Vec keys in a HashMap were hashed by the buffer address (not by the elements), so `HashMap<(u32, Vec<Ty>), u32>` lookups failed
  - after moving the contents out of `*b` (b: Box), b was still dropped and the contents were freed twice
  - the element type of `it.zip(slice)` was not inferred, so `sn == "__array"` mistook `&String` for `&str` (inference now also uses `.iter()`)
