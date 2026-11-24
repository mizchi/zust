# Rust から見る zustscript の変換コスト

Rust でスライスを借用する関数も、TypeScript から呼ぶと配列全体のコピーを伴うことがあります。生成した `.wasm.rs` で分かるのは Wasm 内部の処理です。呼び出し全体のコストを知るには、同じディレクトリの `.js` にある引数と戻り値の変換も読みます。

## スライスを借りる前に、JS が配列をコピーする

内積を求める `dot` の生成 Rust は `fn zs_dot(a: &[f64], b: &[f64]) -> f64` です。本体は二つのスライスを読み、スカラーを返します。ところが JS 側では、呼び出しのたびに次の処理を行います。これは生成された `numeric.js` のうち、引数を準備する部分です。

```javascript
const a$p = $w.__zsc_alloc(BigInt(a.length * 8));
new Float64Array($w.memory.buffer, Number(a$p), a.length).set(a);
const b$p = $w.__zsc_alloc(BigInt(b.length * 8));
new Float64Array($w.memory.buffer, Number(b$p), b.length).set(b);
```

両方が 1,024 要素なら、入力だけで 16 KiB を転送します。同じ二つの配列で 100 回呼べば約 1.56 MiB です。これは要素数から数えた転送量です。関数が終わると JS 側の `finally` が二つの一時領域を解放します。重みなど同じ配列を使い回す場合は、外側の反復も Wasm に置くと、入力を一度渡してそこで複数回計算できます。[内積のソース](https://github.com/mizchi/zust/blob/1061d587bc0188afc5edaf18a49588703266ca1e/crates/zustscript/examples/numeric.zsc#L4-L6) · [境界コードを生成する wasm.rs](https://github.com/mizchi/zust/blob/1061d587bc0188afc5edaf18a49588703266ca1e/crates/zustscript/src/wasm.rs#L290-L298)

`Float32Array` を受け取る SIMD 関数にも転送があります。`relu` は入力を `.set(xs)` でまとめて送り、同じ長さの出力を `.slice()` で JS に取り出します。1,024 要素なら往復 8 KiB です。`relu(dense(...))` のように JS でつなぐと、前段の出力がいったん JS に戻り、次の呼び出しで再び Wasm に入ります。現在の SIMD 関数の公開シグネチャには、Wasm 側の中間バッファを次の関数に渡すハンドルがありません。[SIMD カーネルと呼び出し側の mlp](https://github.com/mizchi/zust/blob/1061d587bc0188afc5edaf18a49588703266ca1e/crates/zustscript/examples/ml.zsc#L26-L28)

## filter と map の間に Vec ができる

`evens_squared` は配列から偶数を選んで二乗します。Rust のイテレータに慣れていると、最後に一度だけ結果を集めるように見える式です。

```zustscript
@wasm export fn evens_squared(xs: Array<i32>) -> Array<i32> {
  xs.filter((x) => x % 2 == 0).map((x) => x * x)
}
```

生成 Rust の本体を改行して読むと、最初の `collect` の直後に次の `iter` があります。

```rust
(xs.iter().copied()
    .filter(|&x| ((x).wrapping_rem(2i32) == 0i32))
    .collect::<Vec<_>>())
    .iter().copied()
    .map(|x| (x).wrapping_mul(x))
    .collect::<Vec<_>>()
```

入力が N 個、そのうち偶数が K 個なら、最初の走査で K 個の中間配列を作り、二度目の走査でその K 個を読み直します。ここを手で最適化するなら、同じ公開関数を `export wasm fn` で書けます。引数、戻り値、本体は zustc が扱う Rust です。

```zustscript
export wasm fn evens_squared(xs: &[i32]) -> Vec<i32> {
    let mut out = Vec::new();
    for &x in xs {
        if x % 2 == 0 {
            out.push(x.wrapping_mul(x));
        }
    }
    out
}
```

このループは偶数を見つけた時点で二乗し、結果の `Vec` に直接追加します。`wrapping_mul` は、二乗が `i32` の範囲を超えたときも元の関数と同じ値を返すために使っています。JS との入出力には引き続きコピーがかかります。コンパイラ側でこの中間配列を省くなら、`lower.rs` の `B::Filter` と `B::Map` がそれぞれ `collect` を生成する箇所が出発点です。[変換ルールの実装](https://github.com/mizchi/zust/blob/1061d587bc0188afc5edaf18a49588703266ca1e/crates/zustscript/src/lower.rs#L1528-L1542)

## 借用した戻り値の寿命は Wasm の外に出せない

`longer(a, b)` は長い方の配列を返します。Wasm 内で呼ぶときは、生成された関数の型が `fn zs_longer<'a>(a: &'a [f64], b: &'a [f64]) -> &'a [f64]` なので、選んだ側をそのまま借用できます。一方、JS に公開するための Rust ラッパーは次のようになります。

```rust
fn __zs_js_zs_longer(a: &[f64], b: &[f64]) -> Vec<f64> {
    zs_longer(a, b).to_vec()
}
```

ラッパーが選んだスライスを所有する `Vec` に移すためにコピーし、JS 側がその中身をさらに新しい配列へコピーします。長さや合計だけが必要なら、その計算まで Wasm 側に置いて数値を返せます。単に既存の二つの JS 配列から長い方を選ぶ用途なら、通常の `export fn` で長さを比べれば、要素を転送せずに済みます。[longer と借用の例](https://github.com/mizchi/zust/blob/1061d587bc0188afc5edaf18a49588703266ca1e/crates/zustscript/examples/owned.zsc#L74-L76) · [所有権解析](https://github.com/mizchi/zust/blob/main/crates/zustscript/src/own.rs)

## 手元で変換結果を読む

リポジトリのルートで次を実行すると、上の例と同じコンパイラ出力を確認できます。Rust と JS を並べて開き、関数名で検索すると、コピーが本体・公開ラッパー・JS のどこにあるかを追えます。

```bash
cargo build --release --bin zustc
for name in numeric owned; do
  cargo run --manifest-path crates/zustscript/Cargo.toml -- \
    "crates/zustscript/examples/$name.zsc" \
    --zustc ./target/release/zustc -o out/costs
done
rg -n 'zs_evens_squared|zs_longer|to_vec' out/costs/*.wasm.rs
rg -A12 'function (dot|longer)\(' out/costs/*.js
```

実行時間を比較するときは、同じ入力で JS から公開関数を呼ぶと転送も測定に入ります。`@wasm` で書いた元の例は `--all-js` で JS 版も生成できます。手書きの `wasm fn` は Rust 本体を Wasm として実行します。[数値計算のベンチマーク](https://github.com/mizchi/zust/blob/main/crates/zustscript/tests/numeric_bench.mjs) · [zustscript 入門に戻る](introduction.md)
