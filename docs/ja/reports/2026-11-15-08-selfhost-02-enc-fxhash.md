# セルフホストの第 2 段階: enc.rs + fxhash.rs

日付: 2026-11-15

- trait の静的呼び出し：`Enc::dec(r)?`・`T::dec(r)`（Self は期待される型・型推論から）、ジェネリックな impl（`impl<T: Enc> Enc for Vec<T>` / `impl<A, B> Enc for (A, B)`）への静的ディスパッチと単相化
- `Trait::m(&x, ..)` / `crate::m::Trait::m(..)`（self を取るメソッド）はメソッド呼び出しとして
- スライス → 配列の `try_into`（`u64::from_le_bytes(b[..8].try_into().unwrap())`、`TryFromSliceError`）、`get(a..b)` / `get_mut(a..b)`（Vec・配列・スライス）
- `S::default()` / `<S>::default()` / `m::S::default()` / ジェネリックな別名 `FxMap::default()`
- `HashMap<K, V, BuildHasherDefault<H>>` / `HashSet<K, S>`（ハッシャの型引数は受け付けて、既定のハッシュを使う。反復順は std と違う）
- `macro_rules!`：`$t:ty { .. }` のように後ろに括弧が続くフラグメント、`$crate`、`crate::m!(..)` の呼び出し
- ユーザ定義の trait をタプルに実装（`impl<A, B> Tr for (A, B)`）
