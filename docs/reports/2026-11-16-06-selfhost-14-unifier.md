# Self-hosting stage 14: splitting out the type inference unifier

Date: 2026-11-16

- `src/solver.rs`: split out of infer.rs the unifier that does not depend on the syntax tree (type variables, shapes `Shape`, union-find, deferral of associated types and bounds,
  `known_g` / `apply` / `resolve`, etc., a little over 900 lines). `Inf` in infer.rs keeps only the syn-specific parts and uses `Solver` through `Deref`.
  Associated type bindings in bounds (`Iterator<Item = T>`) are also read by our own parser
- `src/ctors.rs`: rewrites tuple variants / tuple structs used as values (`.map(Some)` / `.map(Ty::Struct)` / `.map(W)` / `let f = E::V;`) into
  closures (`|__c0| Some(__c0)`) (lexically, reading the number of fields from the enum / struct definitions. Line numbers are not changed)
- Added: `None::<u32>` (a variant with explicit type arguments), inference of the result type of a user `impl Index`, `as_slice()` on arrays
- Fixes:
  - for non-`move` closures, "by value or by reference" was decided for the whole closure. Merely passing a Copy value (p in `f(p)`) made
    the Vec capture be held by value too, and it was freed twice. This is now decided per capture (only what is passed or returned by value is held by value)
  - array size multiplication overflowed with the placeholder length for const generics (trap in debug builds)
