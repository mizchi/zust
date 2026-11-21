# 001: バリアントの修飾と名前付きフィールド

状態: 評価中 (r3)

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
- **決まる方法は 1 つだけ**: 具体的な型との単一化
  - 具体的な型とは、注釈、関数の signature、修飾したバリアント、引数の型
- **その場で決まらないとき**
  1. 引数のないバリアント(`None`)だけは、関数の終わりまで決定を遅らせる
     - 候補つきの型変数にする(整数リテラルの型と同じ仕組み)
     - 後で具体的な型と単一化されれば、それに決まる(戻り値、引数、修飾した他の分岐など)
  2. 引数を持つバリアント(`At { index: 1 }`、`Ok(s)`)は遅らせない。その場の期待する型で決まらなければ、修飾を求める
     - 引数の中の typed array、クロージャ、フィールドの型が、誤った位置でエラーになるのを防ぐため
  3. 関数の終わりまで決まらなければエラー: ``cannot tell which enum `None` is: write `Hit::None` or `Parsed::None` ``
     - LSP のクイックフィックスで修飾を足せる
- **決まっていない値は、使う前に決める**
  - 決まっていない値を match にかける、フィールドを読む、メソッドを呼ぶと、その場で修飾を求めるエラー
  - match のパターンから逆に決めることはしない(パターンの解決と循環するため)
- **誤りの位置**
  - 遅らせて決まったバリアントが原因の型エラーには、「ここで `Parsed::None` に決まった」という注を付ける
  - 決まる前の型は、hover やエラーで候補を並べて `Hit | Parsed` と表示する
- **期待する型が伝わる場所**(列挙する。これ以外では伝わらない)
  - `let x: T = e`、関数の引数、関数の末尾の式、レコードのフィールドの値
    - `return` は言語にないので、末尾の式だけ
  - ブロックの末尾の式、`if` と `match` のすべての分岐(1 つの分岐で決まれば、他の分岐にも効く)
  - 配列リテラルの要素(要素同士でも)、タプルの要素、`==` / `!=` の反対側
  - JSX の属性の値(コンポーネントの props の型から)
  - `x |> f(a)` は、脱糖した後の `f(x, a)` の引数として扱う
  - クロージャの本体(クロージャの戻り値の型が分かっているとき)
  - `At(found)` のように引数のあるバリアントにも、呼び出しの期待する型が届く
- match のパターンでは、照合する値の型で決まる
- バリアント名の一意性の制約はなくなる。**ただし effect の操作は例外**
  - effect の推論は型検査の前に名前だけで行うので、effect の操作名は effect の enum の間で一意のまま
  - 衝突したときは `perform Console::Print(…)`、handler の arm の `Console::Print(s), k =>` と修飾できる
- **同じファイルで、次の同名は宣言の時点でエラー**
  - 関数とバリアント(`fn Label(…) -> Jsx` と `Shape::Label`)
  - 型の名前とバリアント(`type Circle = { … }`、`@nominal type Circle = { … }` と `Shape::Circle`)
  - 修飾で避けるのではなく、名前を変える
  - 理由: effect の推論は名前だけで関数とバリアントを区別する。002 では別名の名前が型から消える
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
- **パターン**
  - `Circle { r, ... }`: 一部だけ取り出し、残りは `...`
    - 002 の spread と同じ記号にそろえた(r2 の決定)
    - Rust 式の `..` を書いたら、直し方(`...` にする)つきのエラー
    - 残りに名前を付ける `...rest` は、まだ入れない(未決事項)
  - `Circle { center: c, r }`: 名前を変えて束縛
  - `Circle { r: 0, ... }`: リテラルとの照合
    - 整数、文字列、bool だけ
    - 浮動小数点のリテラルパターンは、NaN と -0.0 の扱いが直感とずれるので書けない
  - `...` がなく、フィールドを書き漏らしたらエラー
    - メッセージに直し方(`...` を足す)を出す
    - LSP のクイックフィックスにする
- **絞り込み**(r2 の決定: scrutinee の絞り込みと `@` の両方)
  - scrutinee が変数か、フィールドのパス(`st.shape`)なら、腕の中ではそれ自体が絞り込まれる
    - `match s { Circle { center, ... } => Circle { ...s, center: add(center) }, … }`
    - 腕の中では `s.r` も読める
  - `@` は、入れ子の部分に名前を付けるときに使う
    - `Ok(c @ Circle { center, ... }) => …`: `c` は `Shape::Circle`
- **バリアントの型 `Shape::Circle`**
  - 型の位置に書ける: `fn grow(c: Shape::Circle) -> Shape::Circle { Circle { ...c, r: c.r * 2.0 } }`
  - `Shape` の部分型。言語にある部分型はこれだけ(レコードには幅の部分型がない。002)。`Shape` を期待する所なら、どこにでも渡せる(引数、戻り値、配列の要素など)
  - フィールドの読み出し `c.r` と、更新 `Circle { ...c, … }` ができる
  - 絞り込まれた変数の型も、腕の中ではこれになる
- **更新**
  - 002 の spread の規則に従う
    - `...` は先頭に 1 つだけ。後に書いたフィールドが勝つ(JS と同じ)
    - 結果の型は `...c` の型と同じ(`Shape::Circle`)
    - 後のフィールドは、c にあるフィールドを同じ型で上書きするだけ
  - バリアントからの更新には、バリアント名を書く: `Circle { ...c, center: x }`
    - 名前なしの `{ ...c, center: x }` で `c` がバリアントなら、エラーにして `Circle { ...c, center: x }` を示す
    - 名前なしの `{ … }` は 002 のレコードを作る書き方で、バリアントではないため
    - 名前を求めるのは spread の元がバリアントのときだけ(出力に判別キーを書くため)。`@nominal` のレコードは 002 の規則どおり、名前なしでも `...m` の型になる
- 絞り込まれていない値のフィールドの読み出し `s.r` と、更新 `Circle { ...s, … }` は書けない
  - `s` の型は `Shape` で、Circle とは限らないため
- **判別キーとの衝突**: 判別キー(今は `TAG`。表現の決定で変わる)と同じ名前のフィールドはエラー

### C. レコードとの関係(パターン)

- レコードのパターンも同じ書き方: `Vec2 { x, y }`、残りは `Vec2 { x, ... }`
- 型が分かっているところでは、型名を省ける: `let { x, y } = p;`
  - 002 の、引数の位置の分割代入と同じ規則
- レコードのパターンも、`...` なしで書き漏らしたらエラー
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
    Circle { center, ... } => Circle { ...s, center: add(center) },   // s は腕の中で Shape::Circle
    Rect { min, max } => Rect { min: add(min), max: add(max) },
    Label { at, ... } => Label { ...s, at: add(at) },
  }
}

pub fn describe(p: Parsed) -> String {
  match p {
    Ok(s) => "shape with area " + area(s).to_string(),
    None => "nothing",                 // 照合する値の型 Parsed で決まる
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

- `describe_later` は、r2 では「else 側で Hit に決まる」としていた。r3 では、引数を持つ `At` は遅らせないので、その場で修飾を求める
- 遅らせた `None` が後で決まり、それが原因で型が合わないときは、決まった場所に注が付く

```zsc
fn report(p: Parsed) -> () { … }
fn count(h: Hit) -> i32 { … }

fn f() -> i32 {
  let x = None;     // note: `None` was decided to be `Parsed::None` here, by the argument of `report`
  report(x);
  count(x)          // error: expected `Hit`, found `Parsed`
}
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

```ts
export type Shape = Shape.Circle | Shape.Rect | Shape.Label;
export declare namespace Shape {
  type Circle = { readonly TAG: "Circle"; readonly center: Vec2; readonly r: f64 };
  type Rect = { readonly TAG: "Rect"; readonly min: Vec2; readonly max: Vec2 };
  type Label = { readonly TAG: "Label"; readonly at: Vec2; readonly text: string; readonly size: f64 };
}
```

- TS 側では `Shape.Circle` と書いて、1 つのバリアントだけを受けられる(型と同名の namespace)
  - zustscript の型 `Shape::Circle` は、`.d.ts` ではこの `Shape.Circle` になる
- `@wasm` では Rust の struct バリアント `ZsShape::Circle { center, r }` になる
  - 境界のスロットへの分解は、位置引数のバリアントと同じ(`VTy::Enum` にフィールド名を持たせる)
  - 更新は、Rust の enum に `..c` がないので、全フィールドを束縛し直して組み立てる
    - `ZsShape::Circle { center, r } => ZsShape::Circle { center: add(center), r }` のような形

## 実装の注意(implementer の指摘から)

- パターンに Id を付け、解決した enum を side table(`pat_enum`)に記録する。js.rs の `pat_cond` と lower.rs の `pat` は、名前でなくこれを引く
- `EK::Call` の callee がバリアントのときは、呼び出しの期待する型を解決に渡す
- 遅延した解決は関数の中で閉じる(finish_fn で決める)。アイテム単位の増分検査は変わらない
- **finish_fn の順序**
  1. 候補つきの型変数の固定点(バリアントが決まると、フィールドの型との単一化が起き、別の候補が決まることがあるため)
  2. 整数と小数の既定値(i32 / f64)。1 より後にしないと、フィールドの型より先に既定値で固定される
  3. deferred(`==` / `<` などの検査)
  4. casts
  5. 網羅性の検査(`EK::Match` から finish_fn に移す)
  6. side table の書き出し
- 絞り込みは束縛のメタデータとして持つ
  - 腕の中の変数(またはフィールドのパス)に「Circle に絞り込んだ」という印を付ける
  - フィールドの読み出しと spread は、この印を見る
  - 型 `Shape::Circle` は、enum の型にバリアントを添えたもの。`Shape` を期待する所では `Shape` に広げてから単一化する
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

## 未決事項

- 全バリアントに共通するフィールド(すべてに `at` がある)を、絞り込まずに `s.at` で読めるようにするか
- 絞り込みを、`if` の条件(`if s is Circle { … }` のような形)にも広げるか
- 腕の中で `let mut` の変数に代入したとき、絞り込みを外すか(代入をエラーにするか)
- `Array<Shape::Circle>` を `Array<Shape>` に渡せるか(部分型を型引数の中まで広げるか)
- 残りに名前を付ける `Circle { r, ...rest }` を入れるか。入れるなら `rest` の型は何か(TODO)
- 別のファイル(モジュール)の enum のバリアントの解決(import と一緒に決める)
- 総称的な enum の修飾は `Option::None` と書き、型引数は推論する(`Option::<i32>::None` とは書かない)でよいか
- **他へ移したもの**
  - or パターン(`A | B =>`)とガード(`if` つきの腕): 新しい提案 004 で扱う
  - TS 側のコンストラクタ(TS から `Shape.Circle` の値を作る関数)と、別の enum の同名のバリアントが同じ TAG になること(`Parsed.None` と `Hit.None`): 表現の決定(`@tag`)で扱う
  - データなしの enum にデータつきのバリアントを足すと `.d.ts` の形が変わること: 同じく表現の決定で扱う

## 改訂履歴

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
