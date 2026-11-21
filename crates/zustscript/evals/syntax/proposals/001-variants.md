# 001: バリアントの修飾と名前付きフィールド

状態: 評価中 (r4)

## 動機

`corpus/shapes.zsc` より。

1. **フィールドの意味が型に書けない**
   - `Circle(Vec2, f64)` の f64 が半径であることは、コメントにしか書けない
   - `Label(_, _, _)` は、フィールドが増えると全部の match を直すことになる
   - `.d.ts` は `_0` / `_1` / `_2` になり、TS 側で意味が分からない
2. **バリアント名がファイル全体で一意**
   - `Parsed` の「何もない」場合を `None` と書けない(別の enum が `None` を持つと衝突する)
   - そのため `Missing` / `NoHit` のような名前を工夫することになる

この文書では、`(…)` / `{ … }` は「中身を省略した」という意味で使う。コードとしての `...` は、パターンの残りと spread だけ。

## 提案

### A. 修飾と、期待する型による解決

- `Shape::Circle(…)` / `Shape::Circle { … }` と書けば、その enum のバリアント。パターンでも同じ
- 修飾しないバリアント名は、**期待する型**から決める
  - ファイル内で一意かどうかは使わない。r1 で、enum を 1 つ足すと離れた場所が壊れると指摘されたため
  - **候補の数でも決めない**。名前を持つ enum が 1 つだけでも、それだけでは決まらない(r2 の指摘。候補の数で決めると、r1 の規則が裏口から戻るため)
- **決まる方法は 2 つだけ**
  1. 具体的な型との単一化
     - 具体的な型とは、注釈、関数の signature、修飾したバリアント、引数の型
  2. prelude の既定値(下の「prelude の Option / Result」)
- **その場で決まらないとき**
  1. 引数のないバリアント(`None`)だけは、関数の終わりまで決定を遅らせる
     - 候補つきの型変数にする(整数リテラルの型と同じ仕組み)
     - 後で具体的な型と単一化されれば、それに決まる(戻り値、引数、代入、修飾した他の分岐など)
  2. 引数を持つバリアント(`At { index: 1 }`、`Ok(s)`)は遅らせない
     - prelude の `Some` / `Ok` / `Err` なら、その場で prelude の enum として推論する
     - それ以外は、その場の期待する型で決まらなければ、修飾を求める
     - 引数の中の typed array、クロージャ、フィールドの型が、誤った位置でエラーになるのを防ぐため
  3. 関数の終わりまで決まらないとき
     - prelude の `None` なら、prelude の `Option` に決める
     - それ以外はエラー: ``cannot tell which enum `Empty` is: write `Shape::Empty` or `Cell::Empty` ``
     - LSP のクイックフィックスで修飾を足せる
- **prelude の Option / Result**(r3 の決定 5)
  - 対象は、003 で標準にする `Option` / `Result` のバリアント(`None` / `Some` / `Ok` / `Err`)だけ
  - 整数リテラルが、決まらなければ i32 になるのと同じ。決まらなければ prelude の型になる
  - 引数のあるもの(`Some(1)`)は、期待する型がなければ、その場で prelude の enum として推論する
    - 総称なので、引数の型が誤った位置でエラーになる心配はない
    - 期待する型が、まだ決まっていない候補つきの型変数(`let mut best = None` の `best`)のときも、期待する型がないものとして扱う
  - 期待する型が、同じ名前のバリアントを持つユーザーの enum なら、そちらが勝つ(`describe` の `None`)
  - prelude は固定なので、enum を足しても離れた場所の意味は変わらない(r1 の A.1 の問題は戻らない)
  - prelude に決まったことが原因の型エラーには、「期待する型がないので、ここで `Result::Ok` に決まった」という注を付ける
  - ユーザーの enum のバリアントは、今までどおり修飾か期待する型で決める
- **決まっていない値は、使う前に決める**
  - 決まっていない値を match にかける、フィールドを読む、メソッドを呼ぶと、その場で修飾を求めるエラー
  - match のパターンから逆に決めることはしない(パターンの解決と循環するため)
  - 照合する値の型が、候補のない、ふつうの型変数のとき(総称的な関数の中、`|>` で型が後から決まるとき)
    - 修飾されたパターン(`Shape::Circle { … }`)があれば、それで決める
    - なければ、注釈を求める
- **誤りの位置**
  - 遅らせて決まったバリアントが原因の型エラーには、「ここで `Parsed::None` に決まった」という注を付ける
  - 決まる前の型は、hover やエラーで候補を並べて `Option | Hit | Parsed` と表示する
- **期待する型が伝わる場所**(列挙する。これ以外では伝わらない)
  - `let x: T = e` と `let mut x: T = e`、関数の引数、関数の末尾の式、レコードのフィールドの値
    - `return` は言語にないので、末尾の式だけ
  - 代入と複合代入の右辺(`x = e` の e には x の型)
  - メソッドの引数、`perform` の引数
  - handler の arm の本体と、`k(…)` の引数
  - ブロックの末尾の式、`if` と `match` のすべての分岐(1 つの分岐で決まれば、他の分岐にも効く)
  - 配列リテラルの要素(要素同士でも)、タプルの要素、`==` / `!=` の反対側
  - JSX の属性の値(コンポーネントの props の型から)
  - `x |> f(a)` は、脱糖した後の `f(x, a)` の引数として扱う
  - クロージャの本体(クロージャの戻り値の型が分かっているとき)
  - `At(found)` のように引数のあるバリアントにも、呼び出しの期待する型が届く
- match のパターンでは、照合する値の型で決まる
- **effect の操作名**
  - effect の操作名は、effect の enum の間で一意
  - データの enum のバリアントとは、同じ名前でよい
  - 修飾(`perform Console::Print(…)`、handler の arm の `Console::Print(s), k =>`)は書けるが、必須ではない
  - 理由: effect の推論は型検査の前に名前だけで行う。操作名を effect の間で一意にすれば、名前だけで決まる
- **同じ名前の関数とバリアント**(r3 の決定 6)
  - 両方を宣言できる(`fn Circle(…) -> Jsx` と `Shape::Circle`)
  - そのとき、バリアントは修飾したとき(`Shape::Circle`)だけ書ける。式でもパターンでも同じ
  - 修飾しない `Circle` は関数。`<Circle />` も関数
  - 修飾しない `Circle { … }` をパターンに書くと、直し方(`Shape::Circle { … }`)つきのエラー
  - effect の推論には、修飾したパスを渡す。そのため、名前だけで関数とバリアントを取り違えない
- **宣言の時点でエラーにするのは、型の別名とバリアントの同名だけ**
  - `type Circle = { … }` や `@nominal type Circle = { … }` と、同じファイルの `Shape::Circle`
  - 理由
    - 生成もパターンも同じ字面 `Circle { … }` で、どちらもデータなので、修飾しない方がどちらかを覚える規則が要る
    - 002 では別名の名前が型から消えるので、取り違えたときのエラーが読みにくい
  - エラーには、「バリアントの型は `Shape::Circle` と書ける」という直し方を出す(別名の代わりになる)
  - 別の enum のバリアント同士(`Hit::None` と `Parsed::None`)は、今までどおり同じ名前でよい

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
  - 生成した式の型は `Shape`。`Circle` という絞り込みの情報が付く(下の「絞り込み」)
- **パターン**
  - `Circle { r, ... }`: 一部だけ取り出し、残りは `...`
    - 002 の spread と同じ記号にそろえた(r2 の決定)
    - Rust 式の `..` を書いたら、直し方(`...` にする)つきのエラー
    - 残りに名前を付ける `...rest` は、まだ入れない(未決事項)
  - `Circle { center: c, r }`: 名前を変えて束縛
  - `Label { text: "", ... }`: リテラルとの照合
    - 整数、文字列、bool だけ
    - 浮動小数点のリテラルパターンは、NaN と -0.0 の扱いが直感とずれるので書けない
  - match のパターンで、`...` がなく、フィールドを書き漏らしたらエラー
    - メッセージに直し方(`...` を足す)を出す
    - LSP のクイックフィックスにする
- **絞り込み**(r2 の決定: scrutinee の絞り込みと `@` の両方)
  - scrutinee が変数か、フィールドのパス(`st.shape`)なら、腕の中ではそれ自体が絞り込まれる
    - `match s { Circle { center, ... } => Circle { ...s, center: add(center) }, … }`
    - 腕の中では `s.r` も読める
  - `@` は、入れ子の部分に名前を付けるときに使う
    - `Ok(c @ Circle { center, ... }) => …`: `c` は Circle に絞り込まれる
  - 絞り込みは、型ではなく束縛に付く情報(変数 → バリアント)
    - `s` の型は、腕の中でも `Shape` のまま
    - `let d = s` の `d` も絞り込まれる(情報を写す)
  - 絞り込まれた `let mut` の変数に、腕の中で代入するとエラー
    - 直し方: 別の変数に束縛する、または `@` で名前を付ける
    - 後で緩めることはできる(互換を壊さない)
- **型 `Shape::Circle`**
  - 注釈、引数、戻り値の型の位置に書ける: `fn grow(c: Shape::Circle) -> Shape::Circle { Circle { ...c, r: c.r * 2.0 } }`
  - ただし、型検査の中の型(Ty)にはならない。値の型は常に `Shape`
  - `Shape::Circle` と書いた所では、来る値が Circle に絞り込まれているかを確かめる
    - 通るもの: 絞り込まれた変数、`Circle { … }` の生成、戻り値が `Shape::Circle` の関数の呼び出し
    - それ以外はエラー: ``expected `Shape::Circle`, found `Shape` (narrow it with `match`)``
  - 型が `Shape::Circle` の引数と、注釈つきの `let` の変数は、絞り込まれた束縛になる
  - 型引数の中(`Array<Shape::Circle>`)とレコードのフィールドの型には、r4 では書けない(未決事項)
- **単一化に部分型はない**
  - 単一化のほかにある規則は、絞り込みの情報だけ
  - `if b { s } else { t }`、`[c, shape]`、match の腕の結果は、どれも `Shape`。書いた順で結果は変わらない
  - 絞り込みの情報を見るのは、次の 3 つだけ
    - フィールドの読み出し(`c.r`)
    - spread の元(`Circle { ...c, … }`)
    - 期待する型が `Shape::Circle` と書かれた所の検査(注釈、引数、戻り値)
- **更新**
  - 002 の spread の規則に従う
    - `...` は先頭に 1 つだけ。後に書いたフィールドが勝つ(JS と同じ)
    - 結果の型は `Shape`。spread の元と同じ絞り込み(Circle)が付く
    - 後のフィールドは、c にあるフィールドを同じ型で上書きするだけ
  - バリアントからの更新には、バリアント名を書く: `Circle { ...c, center: x }`
    - 名前なしの `{ ...c, center: x }` で `c` がバリアントなら、エラーにして `Circle { ...c, center: x }` を示す
    - 名前なしの `{ … }` は 002 のレコードを作る書き方で、バリアントではないため
    - 名前を求めるのは spread の元がバリアントのときだけ(出力に判別キーを書くため)。`@nominal` のレコードは 002 の規則どおり、名前なしでも `...m` の型になる
  - レコードからバリアントへの spread(`Circle { ...rec, r: 1.0 }`、`rec` は 002 のレコード)はエラー
    - 直し方: フィールドを並べる(`Circle { center: rec.center, r: 1.0 }`)
- 絞り込まれていない値のフィールドの読み出し `s.r` と、更新 `Circle { ...s, … }` は書けない
  - `s` は Circle とは限らないため
- **判別キーとの衝突**: 判別キー(今は `TAG`。表現の決定で変わる)と同じ名前のフィールドはエラー

### C. レコードとの関係(パターン)

- レコードのパターンも同じ書き方: `Vec2 { x, y }`、残りは `Vec2 { x, ... }`
- 型が分かっているところでは、型名を省ける: `let { x, y } = p;`
  - 002 の、引数の位置の分割代入と同じ規則
- フィールドの書き漏らしがエラーになるのは、match のパターンだけ(002 の決定 7)
  - `let` と引数のレコードのパターンは、TS と同じく一部だけでよい: `let { x } = p;`
  - match では、レコードのパターンも `...` が要る
- 同じ名前のレコードの別名とバリアントは、A のとおり宣言の時点でエラー

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
    Circle { r, ... } => 3.141592653589793 * r * r,
    Rect { min, max } => ((max.x - min.x) * (max.y - min.y)).abs(),
    Label { ... } => 0.0,
  }
}

pub fn moved(s: Shape, d: Vec2) -> Shape {
  let add = (p: Vec2) => Vec2 { x: p.x + d.x, y: p.y + d.y };
  match s {
    Circle { center, ... } => Circle { ...s, center: add(center) },   // s は腕の中で Circle に絞り込まれる(型は Shape)
    Rect { min, max } => Rect { min: add(min), max: add(max) },
    Label { at, ... } => Label { ...s, at: add(at) },
  }
}

fn grow(c: Shape::Circle) -> Shape::Circle { Circle { ...c, r: c.r * 2.0 } }

pub fn grow_all(shapes: Array<Shape>) -> Array<Shape> {
  shapes |> .map((s) => match s {
    Circle { ... } => grow(s),     // 通る: s は Circle に絞り込まれている
    _ => s,                        // どの腕も Shape。腕の順序によらない
  })
}

fn grow_first(shapes: Array<Shape>) -> Shape {
  grow(shapes[0])
  //   ^^^^^^^^^ error: expected `Shape::Circle`, found `Shape` (narrow it with `match`)
}

pub fn describe(p: Parsed) -> String {
  match p {
    Ok(s) => "shape with area " + area(s).to_string(),
    None => "nothing",                 // 照合する値の型 Parsed で決まる(prelude の None より優先)
    Bad(why) => "error: " + why,
  }
}

pub fn first_hit(shapes: Array<Shape>, at: Vec2) -> Hit {
  let mut found = -1;
  for i in range(0, shapes.len()) {
    if found < 0 && contains(shapes[i], at) { found = i; }
  }
  if found < 0 { None } else { At { index: found } }   // 戻り値の型 Hit が両方の分岐に伝わる
}

fn last_positive(xs: Array<i32>) -> i32 {
  let mut best = None;                    // 候補: Option | Hit | Parsed
  for i in range(0, xs.len()) {
    if xs[i] > 0 { best = Some(i); }      // 期待する型が決まっていないので、Some(i) は prelude の Option<i32>
  }                                       // best も Option<i32> に決まる
  match best { Some(i) => i, None => -1 }
}

fn describe_later(c: bool) -> i32 {
  let h = if c { None } else { At { index: 1 } };
  //                           ^^ error: cannot tell which enum `At` is: write `Hit::At`
  //                              (quick fix: `Hit::At { index: 1 }`, or annotate `let h: Hit = …`)
  match h { None => 0, At { index } => index }
}

// 直した形(どちらでもよい)
fn describe_later_a(c: bool) -> i32 {
  let h: Hit = if c { None } else { At { index: 1 } };   // 注釈で決まる
  match h { None => 0, At { index } => index }
}
fn describe_later_b(c: bool) -> i32 {
  let h = if c { None } else { Hit::At { index: 1 } };   // 修飾した else 側との単一化で None も決まる
  match h { None => 0, At { index } => index }
}
```

- `describe_later` の `At` は prelude のバリアントではないので、既定値はない。r3 と同じく、その場で修飾を求める
- `last_positive` は、r3 では `Some(i)` に修飾が要った。r4 では prelude の既定値で通る
- if の本体の `{ None }` には、002 の `{ x }` の警告は出ない(block() が直接読む本体のため)
- 遅らせた `None` や prelude の既定値が原因で型が合わないときは、決まった場所に注が付く

```zsc
fn report(p: Parsed) -> () { … }
fn count(h: Hit) -> i32 { … }

fn f() -> i32 {
  let x = None;     // note: `None` was decided to be `Parsed::None` here, by the argument of `report`
  report(x);
  count(x)          // error: expected `Hit`, found `Parsed`
}

fn g(s: Shape) -> String {
  let p = Ok(s);    // note: no expected type here, so `Ok` was decided to be the prelude's `Result::Ok`
  describe(p)       // error: expected `Parsed`, found `Result<Shape, _>`
                    // (quick fix: `Parsed::Ok(s)`, or annotate `let p: Parsed = …`)
}
```

同じ名前のコンポーネント関数があるファイルでは、次のように書く(r3 の決定 6)。

```zsc
fn Circle({ r }: { r: f64 }) -> Jsx { <circle r={r} /> }   // Shape::Circle と同じ名前でよい

fn view(s: Shape) -> Jsx {
  match s {
    Shape::Circle { r, ... } => <Circle r={r} />,   // バリアントは修飾して書く。<Circle /> は関数
    Rect { ... } => <rect />,                       // 同名の関数がないバリアントは、今までどおり
    Label { text, ... } => <text>{text}</text>,
  }
}
// 修飾しない `Circle { r, ... } =>` は error: `Circle` is a function here: write `Shape::Circle`
// このファイルでは、上の area / moved の `Circle` も `Shape::Circle` と書く
```

## 出力

表現(`TAG` をキーにすること)は保留中の決定に従う。この提案が決めるのは次の 3 つ。

- 名前付きフィールドが**名前のまま**プロパティになる
- 更新では、判別キーを spread の後に明示する
- `.d.ts` にバリアントごとの型を出す

```js
// Circle { center: c, r: 2.0 }
({ TAG: "Circle", center: c, r: 2.0 })
// match s { Circle { center, ... } => Circle { ...s, center: add(center) }, … }
if (s.TAG === "Circle") { const center = s.center; return { ...s, TAG: "Circle", center: add(center) }; }
```

- 判別キーを spread の後に書くのは、結果のバリアントが、`...s` の中身によらず字面どおりになるようにするため
- 同じ名前の関数とバリアントは、JS では関数 `Circle` とオブジェクトのリテラルになるので、衝突しない

```ts
export type Shape = Shape.Circle | Shape.Rect | Shape.Label;
export declare namespace Shape {
  type Circle = { readonly TAG: "Circle"; readonly center: Vec2; readonly r: f64 };
  type Rect = { readonly TAG: "Rect"; readonly min: Vec2; readonly max: Vec2 };
  type Label = { readonly TAG: "Label"; readonly at: Vec2; readonly text: string; readonly size: f64 };
}
```

- TS 側では `Shape.Circle` と書いて、1 つのバリアントだけを受けられる(型と同名の namespace)
  - signature に書いた `Shape::Circle` は、`.d.ts` ではこの `Shape.Circle` になる
  - 関数 `Circle` は `export declare function Circle(…)` なので、型の `Shape.Circle` とは衝突しない
- `@wasm` では Rust の struct バリアント `ZsShape::Circle { center, r }` になる
  - 境界のスロットへの分解は、位置引数のバリアントと同じ(`VTy::Enum` にフィールド名を持たせる)
  - 型が `Shape::Circle` の引数と戻り値も、境界では `ZsShape` のまま渡す
  - Rust の enum にはフィールドの読み出しがないので、絞り込んだ変数は腕のパターンで全フィールドを束縛し直す
    - パターンで名前を付けたフィールドはその名前、付けなかったフィールドは `s_r` のような名前にする
    - `s.r` は、その名前に下ろす
    - 腕の中で `s` そのものを値として使うときは、束縛したフィールドから組み立て直す
  - 型が `Shape::Circle` の引数は、本体の先頭で `let … else { unreachable!() }` で分解する
  - 更新は、Rust の enum に `..c` がないので、束縛したフィールドから組み立てる

```rust
// moved の Circle の腕
ZsShape::Circle { center, r: s_r } => ZsShape::Circle { center: add(center), r: s_r },

// fn grow(c: Shape::Circle) -> Shape::Circle { Circle { ...c, r: c.r * 2.0 } }
fn grow(c: ZsShape) -> ZsShape {
    let ZsShape::Circle { center: c_center, r: c_r } = c else { unreachable!() };
    ZsShape::Circle { center: c_center, r: c_r * 2.0 }
}
```

## 実装の注意(implementer の指摘から)

- パターンに Id を付け、解決した enum を side table(`pat_enum`)に記録する。js.rs の `pat_cond` と lower.rs の `pat` は、名前でなくこれを引く
- `EK::Call` の callee がバリアントのときは、呼び出しの期待する型を解決に渡す
- prelude の `None` / `Some` / `Ok` / `Err`
  - 候補つきの型変数の候補に、prelude の enum も入れる
  - 引数のあるものは、期待する型がなければ、その場で prelude の enum の型にする
- 遅延した解決は関数の中で閉じる(finish_fn で決める)。アイテム単位の増分検査は変わらない
- **finish_fn の順序**
  1. 残った候補つきの型変数を片付ける
     - 候補に prelude の enum があれば、それに決める(既定値)
     - なければエラー。決まった場所の注は、単一化のときに記録しておく
  2. 整数と小数の既定値(i32 / f64)
  3. deferred(`==` / `<` などの検査)
  4. casts
  5. side table の書き出し
- **網羅性は `EK::Match` に残す**(照合する値の型は、その場で決まっているか、エラーになっているため)
  - パターン行列(Maranget の方式)で、網羅性と到達可能性を検査する
  - 入れ子のバリアントとリテラルのパターンも扱う(`Ok(Circle { ... })`、`Label { text: "", ... }`)
  - 足りないものは `Ok(Circle { ... })` の形で示す。到達しない腕は警告
- **絞り込みは束縛のメタデータ**(変数またはフィールドのパス → バリアント)
  - Ty にバリアントの型は入れない。単一化はすべて `Shape` で行う
  - メタデータを見るのは、`EK::Field`、spread の元、期待する型が `Shape::Circle` と書かれた所の検査だけ
  - `let d = s` では、メタデータを写す
  - 型の位置の `Shape::Circle` は、(型 `Shape`、絞り込み Circle)の組として読む。型引数の中ならエラー
- **effect の操作の表 `op_of`** を、`variant_of` から分ける
  - EffWalk、`EK::Perform`、`EK::Handle` は `op_of` を引く
  - parse.rs の handler の arm で、`Console::Print` の修飾を読めるようにする(`HArm` に enum 名を持たせる)
- **同じ名前の関数とバリアント**
  - 修飾しない名前は、同じ名前の関数があれば関数として解決する
  - 修飾したバリアントは、パスのまま EffWalk に渡る。EffWalk は関数の表を引かない
- `irrefutable` を checker のメソッドにする(`let { x, y } = p` の判定には型が要る)
- lexer の `PUNCTS` に `...` を足す。前から順に照合するので、`..` より前に置く

## 代替案

1. **TS の判別共用体をそのまま書く**(`type Shape = { kind: "circle", … } | …`)
   - match、網羅性、生成を、レコードと区別しにくい
   - 選ばない(`.d.ts` の形はこれに近づけた)
2. **Swift 風の `.Circle`**
   - 期待する型からの解決を記号で明示する案
   - r1 では支持がなかった(`|> .map` と字面が近い)。選ばない
3. **ファイル内で一意なら修飾を省ける規則**(r1 の A.1): 非局所的に壊れるので廃止した
4. **候補の数で決める遅延**(r2): A.1 と同じ問題が戻るので廃止した
5. **`@` だけで絞り込む**(r2): TS の人は scrutinee 自体が絞り込まれると期待するので、両方にした
6. **`Shape::Circle` を `Shape` の部分型として単一化に入れる**(r3)
   - 単一化は等式だけなので、`if` の分岐や配列の要素で、書いた順によって通るかが変わる
   - 廃止し、絞り込みを束縛のメタデータだけで持つことにした
7. **同じ名前の関数とバリアントを宣言の時点でエラーにする**(r3)
   - UI では、`Shape::Circle` を描くコンポーネントを `Circle` と名付けることが多い
   - 廃止し、バリアントを修飾で書くことにした(r3 の決定 6)

## 未決事項

- 全バリアントに共通するフィールド(すべてに `at` がある)を、絞り込まずに `s.at` で読めるようにするか
- 絞り込みを、`if` の条件(`if s is Circle { … }` のような形)にも広げるか
- `Shape::Circle` を型引数の中(`Array<Shape::Circle>`)やフィールドの型に書けるようにするか
- 型が `Shape::Circle` の値を match にかけたときの網羅性(`Circle` の腕 1 つで足りるとするか)
- 絞り込まれた変数をクロージャが捕まえたとき、クロージャの中でも絞り込まれたままか
- 同じ名前の関数があるとき、パターンでは修飾を省けるようにするか(パターンに関数は来ないため)
- 型の別名とバリアントの同名のエラーを、関数と同じ規則(修飾でなら許す)に緩めるか
- 残りに名前を付ける `Circle { r, ...rest }` を入れるか。入れるなら `rest` の型は何か(TODO)
- 別のファイル(モジュール)の enum のバリアントの解決(import と一緒に決める)
- 総称的な enum の修飾は `Option::None` と書き、型引数は推論する(`Option::<i32>::None` とは書かない)でよいか
- **他へ移したもの**
  - or パターン(`A | B =>`)とガード(`if` つきの腕): 新しい提案 004 で扱う
  - TS 側のコンストラクタ(TS から `Shape.Circle` の値を作る関数)と、別の enum の同名のバリアントが同じ TAG になること(`Parsed.None` と `Hit.None`): 表現の決定(`@tag`)で扱う
  - データなしの enum にデータつきのバリアントを足すと `.d.ts` の形が変わること: 同じく表現の決定で扱う
  - `Option` / `Result` を prelude に入れること自体: 003 で扱う(この提案の既定値は、それを前提にする)

## 改訂履歴

- r4(r3 の指摘と、決定事項から。summary.md の表を参照)
  - 型 `Shape::Circle`
    - 部分型をやめた。`Shape::Circle` は Ty に入れず、値の型は常に `Shape`
    - 絞り込みを束縛のメタデータだけにした。見るのは、フィールドの読み出し、spread の元、`Shape::Circle` と書いた所の検査だけ
    - 「広げてから単一化する」をなくした。`let d = s` は絞り込みを写す、と書いた
    - 生成した式 `Circle { … }` の型は `Shape` と書いた
    - 型引数の中には、r4 では書けないとした
  - 解決の規則
    - prelude の `None` / `Some` / `Ok` / `Err` に既定値を入れた(決定 5)。`last_positive` と `g` の例を足した
    - 同じ名前の関数とバリアントを許し、バリアントは修飾で書くことにした(決定 6)。`view` の例を足した
    - 宣言の時点のエラーは、型の別名とバリアントの同名だけに減らした
    - 期待する型が伝わる場所に、代入、`let mut x: T`、メソッドと `perform` の引数、handler の arm の本体と `k` の引数を足した
    - effect の操作名の規則を 1 つにまとめた(effect の間で一意、データのバリアントとは同名でよい、修飾は任意)
    - 照合する値の型がふつうの型変数のときは、修飾されたパターンで決め、なければ注釈を求める
  - 書き方
    - リテラルパターンの例を `Label { text: "", ... }` に直した
    - レコードからバリアントへの spread をエラーと明記した
    - 絞り込まれた `let mut` への腕の中での代入をエラーにした
    - 書き漏らしのエラーを match のパターンだけにした(002 の決定 7)
  - 出力: lower.rs での絞り込んだ変数の束縛し直しと、`Shape::Circle` の引数の分解を書いた。境界は `ZsShape` のまま
  - 実装の注意: finish_fn の第 1 段を「残った候補を片付ける」にした。網羅性はパターン行列で、`EK::Match` に残す。`op_of` を分ける
- r3(r2 の指摘と、決定事項から。summary.md の表を参照)
  - 解決の規則
    - 決まるのは具体的な型との単一化だけにした。候補の数では決めない
    - 遅らせるのは引数のないバリアントだけにした。引数を持つものは、その場で決まらなければ修飾を求める
    - 決まっていない値の match、フィールドの読み出し、メソッド呼び出しは、その場で修飾を求める
    - `describe_later` の例を、エラーと直し方に書き直した
    - 決まった場所を示す注と、`Hit | Parsed` の表示を足した
    - 期待する型が伝わる場所に、JSX の属性とタプルの要素を足し、`|>` の扱いを書いた。`return` を外した
    - 同じファイルでの、関数とバリアント、レコードの別名とバリアントの同名を、宣言の時点でエラーにした
  - 書き方
    - scrutinee の絞り込みを足した。`@` は入れ子の部分に名前を付けるために残した
    - 型 `Shape::Circle` を書けるようにした(`Shape` の部分型)
    - パターンの残りを `...` に統一した。`..` は直し方つきのエラー
    - 名前なしの `{ ...c, x }`(c がバリアント)をエラーにした
    - 更新は 002 の spread の規則に従うと書いた
  - 出力: 更新で判別キーを spread の後に明示する。lower は全フィールドを束縛し直す
  - 実装の注意: finish_fn の順序、絞り込みを束縛のメタデータにすること、`PUNCTS` の `...`
  - 移したもの: or パターンとガードは 004 へ、TS 側のコンストラクタと TAG の衝突は `@tag` の決定へ
- r2(r1 の指摘から。summary.md の表を参照)
  - 解決の規則
    - ファイル内で一意なら解決する規則を廃止し、期待する型による解決に絞った
    - 期待する型で決まらないものは、関数の終わりまで決定を遅らせる
    - 期待する型が伝わる場所を列挙した
    - effect の操作名は一意のまま
  - 書き方
    - `@` 束縛と絞り込み、`{ ...c, … }` での更新を足した
    - 型名を省いたパターン `let { x, y } = p` を足した
    - 位置引数と名前付きの混在を許した(バリアントの中では混ぜない)
    - 浮動小数点のリテラルパターンをなくした
    - 判別キーとの衝突をエラーにした
  - 出力: `.d.ts` にバリアントごとの型(namespace)を出すようにした
