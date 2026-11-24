# zustscript 入門 ── JavaScript と WebAssembly のためのフロントエンド言語

zustscript は、TypeScript のアプリに組み込める静的型付き言語の試作です。画面を組み立てる JSX も、整数の幅や浮動小数点の精度を指定した計算も、同じ言語で書けます。ふつうの関数は読みやすい JavaScript にコンパイルされ、選んだ関数だけを WebAssembly（Wasm）で動かせます。

TypeScript のコードとは型定義を通じてつながります。たとえば、既存のアプリで使っている座標の型を zustscript に取り込めます。逆に、zustscript で書いた関数を公開すると、コンパイラの `zsc` が ES モジュールと `.d.ts` を生成するので、TypeScript 側でも引数と戻り値の型が分かります。

## 状態を enum で表し、match で読み出す

zustscript の `enum` では、状態ごとに異なるデータを持たせられます。たとえば地図で地点を選ぶ画面なら、選択中のときだけ地点名が必要です。

*選択状態から表示用の文字列を返す関数*

```zustscript
enum Selection { Empty, Selected(String) }

export fn label(s: Selection) -> String {
  match s {
    Empty => "地点を選んでください",
    Selected(name) => name,
  }
}
```

`Selected(name)` の分岐では、`name` に選んだ地点の名前が入ります。たとえば後から「検索中」を表す `Searching` を `Selection` に加えると、`zsc` はこの `match` に分岐が足りないことをコンパイル時に指摘します。

## TypeScript の union をそのまま受け取る

画面の状態をすでに TypeScript で定義しているときは、その型を zustscript に取り込めます。たとえば、地図のデータを取得中かどうかを次の型で表します。

*state.ts*

```typescript
export type State = { kind: "idle" } | { kind: "busy"; jobs: number };
```

*state-label.zsc*

```zustscript
import type { State } from "./state.ts";

export fn pending(state: State) -> f64 {
  match state {
    Idle => 0.0,
    Busy(n) => n,
  }
}
```

JavaScript から `pending({ kind: "busy", jobs: 3 })` と呼ぶと `3` が返ります。`Idle` はタグだけなので括弧を付けず、`Busy(n)` は `jobs` の値を取り出します。値が複数あれば、括弧の中の変数はタグを除いた **TypeScript のフィールド宣言順** に対応します。たとえば `height, width` の順で定義した長方形なら、幅を高さで割る式は `Rect(h, w) => w / h` です。

## 関数ごとに WebAssembly を選ぶ

同じ地図で、クリックした位置が地点の中心から 5 ピクセル以内なら、その地点を選択できるとします。画面側は TypeScript の `Point` をそのまま使い、距離を計算する関数にだけ `@wasm` を付けます。この印が付いた関数を `zsc` が Rust に変換し、zust のコンパイラ `zustc` が Wasm にします。

*model.ts*

```typescript
export type Point = { x: number; y: number };
```

同じディレクトリに `pick.zsc` を置きます。`import type` で読み込んだ `Point` の各フィールドは、zustscript では倍精度浮動小数点数の `f64` になります。

*pick.zsc*

```zustscript
import type { Point } from "./model.ts";

@wasm export fn distance(a: Point, b: Point) -> f64 {
  let dx = a.x - b.x;
  let dy = a.y - b.y;
  (dx * dx + dy * dy).sqrt()
}

export fn can_pick(cursor: Point, mark: Point) -> bool {
  distance(cursor, mark) <= 5.0
}
```

`export` を付けた二つの関数は、どちらも JavaScript から名前で import できます。半径を 8 ピクセルに広げるなら、編集するのは `can_pick` の `5.0` だけです。

## 二つの座標は四つの数値になる

生成された `pick.js` には、クリック判定も、Wasm を呼ぶ関数も入っています。モジュールの初期化と読み込み処理を省いて、二つの関数を抜き出すとこうなります。

*pick.js の抜粋*

```javascript
export function can_pick(cursor, mark) {
  return distance(cursor, mark) <= 5.0;
}

export function distance(a, b) {
  return $w.__zsc_w_distance(a.x, a.y, b.x, b.y);
}
```

`$w` は読み込み済みの Wasm モジュールが公開する関数やメモリです。二つの `Point` は境界で四つの数値に展開されます。呼び出し側はいつもの `{ x, y }` を渡せばよく、点をバイト列に詰めるコードは要りません。生成された `pick.d.ts` でも、引数の型は元の `model.ts` の `Point` を参照します。

境界を越える値によっては転送コストがかかります。たとえば数値の配列を JS から渡すと、その中身を Wasm のメモリにコピーします。地図上の数万点から最も近い点を探すなら、点ごとに距離を問い合わせる方法と、探索ループを丸ごと Wasm に置く方法とで、境界を越える回数が変わります。

## 同じ入力で両方を動かす

Rust（edition 2024 対応）と Node.js 24 以降を用意します。リポジトリを取得し、サンプル用のディレクトリに移動します。

```bash
git clone https://github.com/mizchi/zust.git
cd zust
mkdir points
cd points
```

先ほどの `model.ts` と `pick.zsc` をここに保存します。まず `--all-js` を指定して、距離計算も含めて JavaScript に変換します。

```bash
cargo run --manifest-path ../crates/zustscript/Cargo.toml -- \
  pick.zsc --all-js -o js
```

*try.mjs*

```javascript
const { distance, can_pick } = await import(process.argv[2]);
const mark = { x: 0, y: 0 };
for (const cursor of [{ x: 3, y: 4 }, { x: 0, y: 6 }]) {
  console.log(distance(cursor, mark), can_pick(cursor, mark));
}
```

```bash
node try.mjs ./js/pick.js
```

次に `zustc` をビルドし、同じ `pick.zsc` を `--all-js` を外してコンパイルします。出力先の `wasm/` には、JS と型定義に加えて、中間生成物の `pick.wasm.rs` と実行用の `pick.wasm` ができます。

```bash
cargo build --release --manifest-path ../Cargo.toml --bin zustc
cargo run --manifest-path ../crates/zustscript/Cargo.toml -- \
  pick.zsc --zustc ../target/release/zustc -o wasm
node try.mjs ./wasm/pick.js
```

*どちらの実行でも同じ出力*

```text
5 true
6 false
```

中心からちょうど 5 ピクセルの点は選択でき、6 ピクセルでは外れます。距離計算の実行場所を変えても、画面側が渡す座標と判定式は共通です。リポジトリには、この例を広げて点の列から最近傍を探す関数や、JSX で画面を組み立てる例もあります。

- [図形の面積と最近傍を求める shapes.zsc](https://github.com/mizchi/zust/blob/main/crates/zustscript/examples/shapes.zsc)
- [JSX と Wasm を組み合わせた app.zsc](https://github.com/mizchi/zust/blob/main/crates/zustscript/examples/app.zsc)
- [zustscript の言語仕様と実装状況](https://github.com/mizchi/zust/tree/main/crates/zustscript)
