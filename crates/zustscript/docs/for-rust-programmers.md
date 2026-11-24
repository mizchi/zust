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

*out/peak/peak.wasm.rs の抜粋（改行とインデントを調整）*

```rust
fn zs_peak(xs: &[i32]) -> ZsPeak {
    {
        let mut best: ZsPeak = ZsPeak::Empty;
        for x in xs.iter().copied() {
            best = match best {
                ZsPeak::Empty => ZsPeak::Value(x),
                ZsPeak::Value(n) => if (x > n) {
                    ZsPeak::Value(x)
                } else {
                    ZsPeak::Value(n)
                },
            };
        };
        best
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ZsPeak {
    Empty,
    Value(i32),
}
```

`Array<i32>` はこの関数では `&[i32]` になります。ループは `xs.iter().copied()` で借用したスライスから整数を一つずつ読み、`match` は Rust の `ZsPeak` に対する分岐になります。

## JavaScript と Wasm の境界を読む

JS から呼ぶ `peak` は、Wasm のメモリに入力用の領域を確保し、`.set(xs)` で配列の中身をコピーします。Wasm 側が返したタグを読み、空なら `{ TAG: "Empty" }`、値があればその整数を持つオブジェクトを組み立てます。

*out/peak/peak.js の抜粋（改行とインデントを調整）*

```javascript
export function peak(xs) {
  const xs$p = $w.__zsc_alloc(BigInt(xs.length * 4));
  new Int32Array($w.memory.buffer, Number(xs$p), xs.length).set(xs);
  try {
    $w.__zsc_w_peak($out, xs$p, BigInt(xs.length));
    const $o = new DataView($w.memory.buffer, Number($out), 16);
    return (Number($o.getBigInt64(0, true)) === 0
      ? { TAG: "Empty" }
      : { TAG: "Value", _0: Number($o.getBigInt64(8, true)) });
  } finally {
    $w.__zsc_free(xs$p);
  }
}
```

## TypeScript から enum を読む

*out/peak/peak.d.ts の抜粋（改行とインデントを調整）*

```typescript
export type Peak =
  { readonly TAG: "Empty" }
  | { readonly TAG: "Value"; readonly _0: i32 };

export declare function peak(xs: readonly i32[]): Peak;
```

`TAG` を調べると、TypeScript は `"Value"` の分岐でだけ整数フィールド `_0` を読めるようにします。引数の各要素には、同じモジュールが公開する `i32` 変換関数を使います。

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

## 通常の関数を Rust モジュールとして使う

`@wasm` は通常のビルドで Rust 経由の Wasm 実行を必須にする属性です。Rust ソースだけ欲しい場合は、普通の `export fn` にも `--emit-rust` が使えます。たとえば `peak.zsc` の `@wasm export fn peak` を `export fn peak` に変えて、次を実行します。

```bash
cargo run --manifest-path crates/zustscript/Cargo.toml -- peak.zsc --emit-rust -o out/native
rustc --edition=2024 --crate-type lib out/native/peak.rs -o out/native/libpeak.rlib
```

`out/native/peak.rs` には `pub fn zs_peak(xs: &[i32]) -> ZsPeak` と `pub enum ZsPeak` が生成されます。Rust から直接呼ぶ場合はスライスをそのまま渡せます。この出力ではモジュール内の関数を Rust に変換するため、JS の extern 呼び出しや JSX などの非対応構文はソース位置付きのエラーになります。

同じ本体を JS で実行するときは属性のない `export fn` を使います。`@wasm` を残したまま `--all-js` を指定すると、対象の関数名と位置が報告されます。所有権やバッファ確保を自分で指定したい関数は、`export wasm fn` で、zustc が扱える Rust のサブセットを本体に書けます。生成コードと手書きのループを比べる例は次の記事にあります。[Rust から見る変換コスト](rust-costs.md) · [zustscript 入門](introduction.md)
