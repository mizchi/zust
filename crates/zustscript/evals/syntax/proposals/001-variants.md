# 001: バリアントの修飾と名前付きフィールド

状態: 草案

## 動機

`corpus/shapes.zsc` より。

1. **フィールドの意味が型に書けない**
   - `Circle(Vec2, f64)` の f64 が半径であることは、コメントにしか書けない
   - パターン `Label(_, _, _)` は、フィールドが増えると全部の match を直すことになる
   - `.d.ts` は `_0` / `_1` / `_2` になり、TS 側で意味が分からない
2. **バリアント名がファイル全体で一意**
   - `Parsed` の「何もない」場合を `None` と書けない(別の enum が `None` を持つと衝突する)
   - そのため `Missing` / `NoHit` のような名前を工夫することになる。`Hit` は enum と同じ名前のバリアント `Hit(i32)` を持たざるを得なかった

## 提案

### A. 修飾と、期待する型による解決

- `Shape::Circle(..)` と書けば、その enum のバリアント。パターンでも同じ(`Shape::Circle(c, r) => ..`)
- 修飾しない `Circle(..)` は、次の順に解決する
  1. そのファイルで `Circle` を持つ enum が 1 つだけなら、それ
  2. 複数あるなら、**期待する型**(`let` の注釈、引数、戻り値、match の照合する値の型)が、そのうちの 1 つならそれ
  3. それでも決まらなければエラー: ``ambiguous variant `None`: `Option::None` or `Parsed::None`? (write the enum)``
- バリアント名の一意性の制約はなくなる
- match のパターンは常に照合する値の型が分かるので、修飾はほぼ要らない

### B. 名前付きフィールドを持つバリアント

```zsc
enum Shape {
  Circle { center: Vec2, r: f64 },
  Rect { min: Vec2, max: Vec2 },
  Label { at: Vec2, text: String, size: f64 },
  Empty,                       // フィールドなし
  Pair(Vec2, Vec2),            // 位置引数のバリアントも残る(混在できる)
}
```

- 生成: レコードと同じ書き方 `Circle { center: c, r: 2.0 }`。省略形 `Circle { center, r }`(同名の変数)
- パターン:
  - `Circle { r, .. }`: 一部だけ、残りは `..`
  - `Circle { center: c, r }`: 名前を変えて束縛
  - `Circle { r: 0.0, .. }`: リテラルとの照合
  - `..` がなく、フィールドを書き漏らしたら、エラーにする(フィールドの追加に気づける)
- 更新 `Circle { r: 2.0, ..s }` は書けない(`s` の型は `Shape` で、`Circle` だとは限らない)。match の中で束縛し直す
- フィールドの読み出し `s.r` は書けない(どのバリアントか分からない)。match で取り出す

### 同じ規則がレコードにも効く(おまけ)

- レコードのパターン `Vec2 { x, y }` を、match と `let` の両方で書けるようにする
- 例: `let Vec2 { x, y } = p;`。レコードは常に一致するので、`let` に書ける

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
enum Parsed { Ok(Shape), None, Bad(String) }      // None が使える
enum Hit { None, At(i32) }                         // 同じく

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
    Circle { center, r } => Circle { center: add(center), r },
    Rect { min, max } => Rect { min: add(min), max: add(max) },
    Label { at, text, size } => Label { at: add(at), text, size },
  }
}

pub fn first_hit(shapes: Array<Shape>, at: Vec2) -> Hit {
  let mut found = -1;
  for i in range(0, shapes.len()) {
    if found < 0 && contains(shapes[i], at) { found = i; }
  }
  if found < 0 { None } else { At(found) }   // 戻り値の型 Hit から Hit::None に決まる
}
```

## 出力

表現(`TAG` をキーにすること)は保留中の決定に従う。提案は、名前付きフィールドが **名前のまま** プロパティになることだけを決める。

```js
// Circle { center: c, r: 2.0 }
({ TAG: "Circle", center: c, r: 2.0 })
// match の Circle { r, .. }
if (s.TAG === "Circle") { const r = s.r; ... }
```

```ts
export type Shape =
  | { readonly TAG: "Circle"; readonly center: Vec2; readonly r: f64 }
  | { readonly TAG: "Rect"; readonly min: Vec2; readonly max: Vec2 }
  | { readonly TAG: "Label"; readonly at: Vec2; readonly text: string; readonly size: f64 };
export type Hit = { readonly TAG: "None" } | { readonly TAG: "At"; readonly _0: i32 };
```

- `@wasm` に落とすとき
  - Rust では struct バリアント `ZsShape::Circle { center, r }` になる
  - 境界のスロットへの分解は、位置引数のバリアントと同じ

## 代替案

1. **TS の判別共用体をそのまま書く**: `type Shape = { kind: "circle", center: Vec2, r: f64 } | { kind: "rect", .. }`
   - TS 開発者にはそのまま読める
   - 一方で、match のパターン、網羅性、生成の書き方が、レコードと区別しにくい
   - 判別キーを何にするかは、表現の決定(`@tag`)と重なる
   - 選ばない。ただし `.d.ts` の形はこれに近づける
2. **Swift 風の暗黙のメンバー `.Circle { .. }`**: 修飾しないときは必ず `.` を付け、期待する型から解決する
   - ファイル内で一意かどうかで意味が変わらないので、規則が単純
   - ただし `.Circle` は `|> .map(..)` のメソッド呼び出しの記法と字面が近い
   - 選ばない(評価で意見を聞きたい)
3. **一意性の制約を残し、修飾だけを足す**: 実装は一番楽だが、動機 2 が解決しない

## 未決事項

- 位置引数のバリアントと名前付きのバリアントの混在を許すか(許すと、生成とパターンの書き方が 2 通りになる)
- 期待する型による解決を、どこまで効かせるか
  - 関数の引数、`==` の右辺、配列リテラルの要素など
  - 効かせる場所が多いほど便利だが、エラーの位置が分かりにくくなる
- 名前付きのバリアントで、`Circle { center, r }` の省略形が、同じ名前のレコード型の生成と区別できるか
  - 名前空間は 1 つにして、レコードとバリアントで同じ名前を禁止する案
- エラー回復: 未完成の `Circle {` を書いている途中の補完(バリアントのフィールドを候補に出す)

## 改訂履歴

(まだなし)
