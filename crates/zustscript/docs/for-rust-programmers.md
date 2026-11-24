# Rust プログラマーのための zustscript

Rust で `xs.iter().copied().max()` と書くと、空のスライスも扱える `Option<i32>` が返ります。zustscript でも「値があるか」を `enum` で表し、`match` で両方の場合を扱えます。この関数を TypeScript に公開するとき、配列の借用と enum の戻り値はどう見えるでしょうか。

## 最大値を返す関数を書く

リポジトリのルートに次の内容で `peak.zsc` を作ります。`Peak` は、この例で使う二つの状態を定義した型です。最初に読んだ値を候補にすれば、負数だけの入力でも正しい最大値を選べます。

```zustscript
enum Peak { Empty, Value(i32) }

@wasm export fn peak(xs: Array<i32>) -> Peak {
  let mut best = Empty;
  for x in xs {
    best = match best {
      Empty => Value(x),
      Value(n) => if x > n { Value(x) } else { Value(n) },
    };
  }
  best
}
```

`let mut`、網羅性を検査する `match`、ブロックの最後の式を返す規則は Rust と共通です。バリアントは `Value(n)` と書けます。モジュールの外に関数を公開するキーワードは `export` です。

## 生成された Rust で借用を見る

`@wasm` を付けた関数は、Rust で実装された `zsc` が型を検査して Rust ソースを生成し、`zustc` がその Rust を検査して Wasm にコンパイルします。リポジトリのルートで次を実行してください。

```bash
cargo build --release --bin zustc
cargo run --manifest-path crates/zustscript/Cargo.toml -- \
  peak.zsc --zustc ./target/release/zustc -o out/peak
```

`out/peak/peak.wasm.rs` を開くと、関数の引数は `fn zs_peak(xs: &[i32]) -> ZsPeak` です。元の宣言は `Array<i32>` ですが、本体は要素を読むだけなので、コンパイラはスライスを借用し、`xs.iter().copied()` で整数を一つずつ読みます。この借用が効くのは Wasm の中です。JS から渡す配列は、呼び出し時に Wasm のメモリへコピーされます。

## TypeScript から enum を読む

`out/peak/peak.d.ts` には、値のある側だけが `_0` フィールドを持つ判別 union が生成されます。TS 側は `TAG` を見ると型を絞り込めます。`i32` はモジュールが公開する数値変換関数で、引数の整数型を TypeScript にも伝えます。

*リポジトリのルートに置く try-peak.ts*

```typescript
import { i32, peak } from "./out/peak/peak.js";

const best = peak([i32(-7), i32(-2), i32(-5)]);
if (best.TAG === "Value") {
  console.log(best._0);
}
console.log(peak([]).TAG);
```

Node.js 24 で `node try-peak.ts` を実行すると、順に `-2` と `Empty` が表示されます。

同じ関数を JS で試すには、`zsc` に `--all-js` を渡します。所有権やバッファ確保を自分で指定したい関数は、`export wasm fn` で、zustc が扱える Rust のサブセットを本体に書けます。生成コードと手書きのループを比べる例は次の記事にあります。[Rust から見る変換コスト](rust-costs.md) · [zustscript 入門](introduction.md)
