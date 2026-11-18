# Self-hosting stage 2: enc.rs + fxhash.rs

Date: 2026-11-15

- Static trait calls: `Enc::dec(r)?`, `T::dec(r)` (Self from the expected type / type inference), static dispatch and monomorphization for generic impls (`impl<T: Enc> Enc for Vec<T>` / `impl<A, B> Enc for (A, B)`)
- `Trait::m(&x, ..)` / `crate::m::Trait::m(..)` (methods that take self) are treated as method calls
- Slice → array `try_into` (`u64::from_le_bytes(b[..8].try_into().unwrap())`, `TryFromSliceError`), `get(a..b)` / `get_mut(a..b)` (Vec, arrays, slices)
- `S::default()` / `<S>::default()` / `m::S::default()` / the generic alias `FxMap::default()`
- `HashMap<K, V, BuildHasherDefault<H>>` / `HashSet<K, S>` (the hasher type argument is accepted, and the default hash is used; iteration order differs from std)
- `macro_rules!`: fragments followed by brackets, like `$t:ty { .. }`, `$crate`, calls of the form `crate::m!(..)`
- Implementing user-defined traits for tuples (`impl<A, B> Tr for (A, B)`)
