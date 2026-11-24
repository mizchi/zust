# TypeScript プログラマーのための zustscript

TypeScript アプリのイベント型はそのままに、集計関数を一つだけ zustscript で書けます。たとえば入庫と出庫を表す判別 union があれば、アプリは今までどおりオブジェクトの配列を渡し、zustscript 側がイベントの種類ごとに在庫の増減を計算します。

## TypeScript の判別 union を使う

次の二つのファイルをリポジトリのルートに置きます。イベントの型は TypeScript で定義します。

*warehouse.ts*

```typescript
export type StockEvent =
  | { kind: "received"; units: number }
  | { kind: "shipped"; units: number };
```

*stock.zsc*

```zustscript
import type { StockEvent } from "./warehouse.ts";

fn delta(event: StockEvent) -> f64 {
  match event {
    Received(n) => n,
    Shipped(n) => -n,
  }
}

export fn stock_change(events: Array<StockEvent>) -> f64 {
  events.fold(0.0, (total, event) => total + delta(event))
}
```

TypeScript の `switch (event.kind)` に相当するのが `match event` です。`Received(n)` は `kind: "received"` のケースを選び、`units` の値を `n` に束縛します。たとえば `warehouse.ts` の union に `{ kind: "returned"; units: number }` を加えると、次のコンパイルで `delta` に分岐が足りないと分かります。

配列の `fold(初期値, 関数)` は TypeScript の `reduce(関数, 初期値)` に対応します。ここでは合計だけを更新し、呼び出し側の配列には書き込みません。TypeScript の `number` フィールドは zustscript で `f64` として読めるので、集計の初期値も浮動小数点の `0.0` にしています。

## 生成された JavaScript と型定義を読む

リポジトリのルートで次を実行します。通常の `export fn` からは JavaScript と型定義が生成されます。補助関数の `delta` はこのモジュール内だけで使えます。

```bash
cargo run --manifest-path crates/zustscript/Cargo.toml -- stock.zsc -o out/stock
```

*out/stock/stock.js の抜粋*

```javascript
import { $fold } from "./zsc-runtime.js";
function delta(event) {
  if (event.kind === "received") {
    const n = event.units;
    return n;
  } else {
    const n$1 = event.units;
    return -n$1;
  }
}

export function stock_change(events) {
  return $fold(events, 0.0, (total, event) => total + delta(event));
}
```

`match` は `event.kind` を調べる `if` に変わります。どちらの分岐も元のオブジェクトの `units` を直接読みます。`events.fold(...)` の呼び出し先は、生成されたランタイムから読み込む `$fold` です。

*out/stock/stock.d.ts の抜粋*

```typescript
export declare function stock_change(events: readonly StockEvent[]): f64;
```

`StockEvent` は元の `warehouse.ts` から取り込んだ型です。公開シグネチャでは `Array<StockEvent>` が `readonly StockEvent[]` になるため、アプリが持っている読み取り専用配列をそのまま渡せます。

## TypeScript アプリから呼ぶ

*同じディレクトリに置く try-stock.ts*

```typescript
import type { StockEvent } from "./warehouse.ts";
import { stock_change } from "./out/stock/stock.js";

const events: readonly StockEvent[] = [
  { kind: "received", units: 12 },
  { kind: "shipped", units: 5 },
];

console.log(stock_change(events));
console.log(stock_change([]));
```

Node.js 24 で `node try-stock.ts` を実行すると、順に `7` と `0` が表示されます。

画面の状態やイベントはこのように JS のモジュールで扱えます。配列の計算などを Wasm で試したくなったら、対応する関数に `@wasm` を付けます。座標を渡す例で、TypeScript 側の呼び出し方と Wasm に渡る値を確認できます。[zustscript 入門](introduction.md) · [Rust から見る変換コスト](rust-costs.md)
