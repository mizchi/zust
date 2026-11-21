# 001: バリアントの修飾と名前付きフィールド

状態: 評価中 (r2)

## 動機

`corpus/shapes.zsc` より。

1. **フィールドの意味が型に書けない**
   - `Circle(Vec2, f64)` の f64 が半径であることは、コメントにしか書けない
   - `Label(_, _, _)` は、フィールドが増えると全部の match を直すことになる
   - `.d.ts` は `_0` / `_1` / `_2` になり、TS 側で意味が分からない
2. **バリアント名がファイル全体で一意**
   - `Parsed` の「何もない」場合を `None` と書けない(別の enum が `None` を持つと衝突する)
   - そのため `Missing` / `NoHit` のような名前を工夫することになる

## 提案

### A. 修飾と、期待する型による解決

- `Shape::Circle(..)` / `Shape::Circle { .. }` と書けば、その enum のバリアント。パターンでも同じ
- 修飾しないバリアント名は、**期待する型**から決める
  - ファイル内で一意かどうかは使わない。r1 で、enum を 1 つ足すと離れた場所が壊れると指摘されたため
  1. 期待する型がその名前のバリアントを持つ enum なら、それ
  2. その場で決まらなければ、関数の終わりまで決定を遅らせる(候補つきの型変数。整数リテラルの型と同じ仕組み)。後の使い方(else 側、戻り値、引数、match)で決まればそれ
  3. それでも決まらなければエラー: ``cannot tell which enum `None` is: write `Hit::None` or `Parsed::None` ``
     - 名前を持つ enum が 1 つだけでも、修飾を求める
     - LSP のクイックフィックスで修飾を足せる
- **期待する型が伝わる場所**(列挙する。これ以外では伝わらない)
  - `let x: T = e`、関数の引数、`return e` と関数の末尾の式、レコードのフィールドの値
  - ブロックの末尾の式、`if` と `match` のすべての分岐(1 つの分岐で決まれば、他の分岐にも効く)
  - 配列リテラルの要素(要素同士でも)、`==` / `!=` の反対側
  - クロージャの本体(クロージャの戻り値の型が分かっているとき)
  - `At(found)` のように引数のあるバリアントにも、呼び出しの期待する型が届く
- match のパターンでは、照合する値の型で決まる
- バリアント名の一意性の制約はなくなる。**ただし effect の操作は例外**
  - effect の推論は型検査の前に名前だけで行うので、effect の操作名は effect の enum の間で一意のまま
  - 衝突したときは `perform Console::Print(..)`、handler の arm の `Console::Print(s), k =>` と修飾できる
- 同じ名前のコンポーネント関数(`fn Label(..) -> Jsx`)とバリアントがあるときは、期待する型が enum ならバリアントを優先する

### B. 名前付きフィールドを持つバリアント

```zsc
enum Shape {
  Circle { center: Vec2, r: f64 },
  Rect { min: Vec2, max: Vec2 },
  Label { at: Vec2, text: String, size: f64 },
  Empty,                       // フィールドなし
  Pair(Vec2, Vec2),            // 位置引数のバリアントも、同じ enum の中に置ける
}
```

- 1 つのバリアントの中で、位置引数と名前付きを混ぜることはできない(Rust と同じ)
- **生成**
  - レコードと同じ書き方: `Circle { center: c, r: 2.0 }`
  - 省略形: `Circle { center, r }`(同名の変数)
- **パターン**
  - `Circle { r, .. }`: 一部だけ取り出し、残りは `..`
  - `Circle { center: c, r }`: 名前を変えて束縛
  - `Circle { r: 0, .. }`: リテラルとの照合
    - 整数、文字列、bool だけ
    - 浮動小数点のリテラルパターンは、NaN と -0.0 の扱いが直感とずれるので書けない
  - `..` がなく、フィールドを書き漏らしたらエラー
    - メッセージに直し方(`..` を足す)を出す
    - LSP のクイックフィックスにする
- **`@` 束縛と絞り込み**
  - `c @ Circle { center, .. }` と書くと、腕の中で `c` は Circle に絞り込まれた値になる
  - 絞り込んだ値は、更新に使える: `Circle { ...c, center: add(center) }`
  - 更新の書き方は 002 と同じ。`...` が先頭で、後に書いたフィールドが勝つ(JS と同じ)
- 絞り込まれていない値のフィールドの読み出し `s.r` と、更新 `Circle { ...s, .. }` は書けない
  - `s` の型は `Shape` で、Circle とは限らないため
- **判別キーとの衝突**: 判別キー(今は `TAG`。表現の決定で変わる)と同じ名前のフィールドはエラー

### C. レコードとの関係(パターン)

- レコードのパターンも同じ書き方: `Vec2 { x, y }`
- 型が分かっているところでは、型名を省ける: `let { x, y } = p;`
  - 002 の、引数の位置の分割代入と同じ規則
- レコードのパターンも、`..` なしで書き漏らしたらエラー
- 同じ名前のレコード型とバリアントがあるときは、バリアントと同じく期待する型で決める。決まらなければ修飾を求める

## 変更前 / 変更後(corpus/shapes.zsc)

```zsc
// 変更前
enum Shape { Circle(Vec2, f64), Rect(Vec2, Vec2), Label(Vec2, String, f64) }
enum Parsed { Ok(Shape), Missing, Bad(String) }
enum Hit { NoHit, Hit(i32) }

pub fn moved(s: Shape, d: Vec2) -> Shape {
  let add = (p: Vec2) => Vec2 { x: p.x + d.x, y: p.y + d.y };
  match s {
    Circle(c, r) => Circle(add(c), r),
    Rect(a, b) => Rect(add(a), add(b)),
    Label(at, text, size) => Label(add(at), text, size),
  }
}
```

```zsc
// 変更後
enum Shape {
  Circle { center: Vec2, r: f64 },
  Rect { min: Vec2, max: Vec2 },
  Label { at: Vec2, text: String, size: f64 },
}
enum Parsed { Ok(Shape), None, Bad(String) }
enum Hit { None, At { index: i32 } }

pub fn area(s: Shape) -> f64 {
  match s {
    Circle { r, .. } => 3.141592653589793 * r * r,
    Rect { min, max } => ((max.x - min.x) * (max.y - min.y)).abs(),
    Label { .. } => 0.0,
  }
}

pub fn moved(s: Shape, d: Vec2) -> Shape {
  let add = (p: Vec2) => Vec2 { x: p.x + d.x, y: p.y + d.y };
  match s {
    c @ Circle { center, .. } => Circle { ...c, center: add(center) },
    Rect { min, max } => Rect { min: add(min), max: add(max) },
    l @ Label { at, .. } => Label { ...l, at: add(at) },
  }
}

pub fn first_hit(shapes: Array<Shape>, at: Vec2) -> Hit {
  let mut found = -1;
  for i in range(0, shapes.len()) {
    if found < 0 && contains(shapes[i], at) { found = i; }
  }
  if found < 0 { None } else { At { index: found } }   // 戻り値の型 Hit が両方の分岐に伝わる
}

fn describe_later(c: bool) -> i32 {
  let h = if c { None } else { At { index: 1 } };   // then 側だけでは決まらず、else 側で Hit に決まる
  match h { None => 0, At { index } => index }
}
```

## 出力

表現(`TAG` をキーにすること)は保留中の決定に従う。この提案が決めるのは次の 2 つ。

- 名前付きフィールドが**名前のまま**プロパティになる
- `.d.ts` にバリアントごとの型を出す

```js
// Circle { center: c, r: 2.0 }
({ TAG: "Circle", center: c, r: 2.0 })
// c @ Circle { center, .. } => Circle { ...c, center: add(center) }
if (s.TAG === "Circle") { const c = s; const center = s.center; return { ...c, center: add(center) }; }
```

```ts
export type Shape = Shape.Circle | Shape.Rect | Shape.Label;
export declare namespace Shape {
  type Circle = { readonly TAG: "Circle"; readonly center: Vec2; readonly r: f64 };
  type Rect = { readonly TAG: "Rect"; readonly min: Vec2; readonly max: Vec2 };
  type Label = { readonly TAG: "Label"; readonly at: Vec2; readonly text: string; readonly size: f64 };
}
```

- TS 側では `Shape.Circle` と書いて、1 つのバリアントだけを受けられる(型と同名の namespace)
- `@wasm` では Rust の struct バリアント `ZsShape::Circle { center, r }` になる
  - 境界のスロットへの分解は、位置引数のバリアントと同じ(`VTy::Enum` にフィールド名を持たせる)

## 実装の注意(implementer の r1 の指摘から)

- パターンに Id を付け、解決した enum を side table(`pat_enum`)に記録する。js.rs の `pat_cond` と lower.rs の `pat` は、名前でなくこれを引く
- `EK::Call` の callee がバリアントのときは、呼び出しの期待する型を解決に渡す
- 遅延した解決は関数の中で閉じる(finish_fn で決める)。アイテム単位の増分検査は変わらない
- `irrefutable` を checker のメソッドにする(`let { x, y } = p` の判定には型が要る)

## 代替案

1. **TS の判別共用体をそのまま書く**(`type Shape = { kind: "circle", .. } | ..`)
   - match、網羅性、生成を、レコードと区別しにくい
   - 選ばない(`.d.ts` の形はこれに近づけた)
2. **Swift 風の `.Circle`**
   - 期待する型からの解決を記号で明示する案
   - r1 では支持がなかった(`|> .map` と字面が近い)。選ばない
3. **ファイル内で一意なら修飾を省ける規則**(r1 の A.1): 非局所的に壊れるので廃止した

## 未決事項

- 全バリアントに共通するフィールド(すべてに `at` がある)を、絞り込まずに `s.at` で読めるようにするか
- `@` 束縛の絞り込みを、`if` の条件(`if s is Circle { .. }` のような形)にも広げるか
- 別のファイル(モジュール)の enum のバリアントの解決(import と一緒に決める)
- 総称的な enum の修飾は `Option::None` と書き、型引数は推論する(`Option::<i32>::None` とは書かない)でよいか

## 改訂履歴

- r2(r1 の指摘から。summary.md の表を参照)
  - 解決の規則
    - ファイル内で一意なら解決する規則を廃止し、期待する型による解決に絞った
    - 期待する型で決まらないものは、関数の終わりまで決定を遅らせる
    - 期待する型が伝わる場所を列挙した
    - effect の操作名は一意のまま
  - 書き方
    - `@` 束縛と絞り込み、`{ ...c, .. }` での更新を足した
    - 型名を省いたパターン `let { x, y } = p` を足した
    - 位置引数と名前付きの混在を許した(バリアントの中では混ぜない)
    - 浮動小数点のリテラルパターンをなくした
    - 判別キーとの衝突をエラーにした
  - 出力: `.d.ts` にバリアントごとの型(namespace)を出すようにした
