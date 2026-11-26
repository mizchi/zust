# zustscript (試作)

zust を土台にした、フロントエンド向け言語の試作。目標は、表面は TS 風の高水準な言語で、必要な箇所だけ Rust と同じ低レベルで書け、境界(コールバックの寿命を含む)をコンパイラが面倒を見ること。

[日本語の入門記事](docs/introduction.md) · [Rust プログラマー向け](docs/for-rust-programmers.md) · [TypeScript プログラマー向け](docs/for-typescript-programmers.md) · [Rust から見る変換コスト](docs/rust-costs.md) · [GitHub Pages で読む](https://mizchi.github.io/zust/zustscript/)

- `zsc x.zsc -o out/` は次を書き出す
  - `x.js`: 読める ES module。JS 層の本体と、wasm 層のグルー
  - `x.d.ts`
  - `x.wasm.rs` と `x.wasm`: Rust 経由の Wasm 関数があるときに生成する。
- `zsc x.zsc --emit-rust -o out/` はモジュール全体を `out/x.rs` に変換する。属性のない関数も対象で、`export fn f` は `pub fn zs_f` になる。標準の Rust で使う例は [Rust プログラマー向けガイド](docs/for-rust-programmers.md) にある。
- **zust の改善を妨げないように、独立させている**
  - `crates/zustscript` に置いているが、zust のワークスペースには入れない(`[workspace]` を自前で持つ)。zust の `cargo build/test --workspace` には含まれない
  - CI も独立した workflow(`.github/workflows/zustscript.yml`)。`crates/zustscript/**` が変わったときだけ動き、zust の CI と KPI には関わらない
  - zustc は外部コマンドとして呼ぶだけ。zust の crate には依存しない
  - 計画メモ: [docs/plan.md](docs/plan.md)
  - `--check-externs`: 各 `extern` の宣言が、実際の TS の .d.ts の型に合うかを tsrs(`npm i tsrs`、または `TSRS`)で確かめる。合わなければ、その `extern` の位置でエラーになる
  - JS ターゲットのサイズと速さの計測: `node bench/js/kpi.mjs --esbuild PATH`(`--against bench/js/baseline.json` で比べる)
  - あとで決めることと、次にやること: [TODO.md](TODO.md)(文法の提案は [evals/syntax/](evals/syntax/README.md))
- ブラウザで試すプレイグラウンド: [editor/](../../editor/README.md)
  - tree-sitter でハイライトし、wasm にした zsc で編集のたびにトランスパイルして、iframe で `main` を実行する(JSX を返せば React で描画)
  - ブラウザでは通常の関数を JS で実行する。`@wasm` は Rust バックエンドを必要とするため、対象の宣言位置でエラーになる。
- テスト: `cargo test --release`
  - **Node 26** を使う(PATH 上の `node`。計測も Node 26.13.1。relaxed SIMD はフラグなしで動く)
  - app の例は zust 側の `target/release/zustc` を使う
  - `.d.ts` の検査は tsc を使う(環境変数 `TSC` か PATH 上の `tsc`。なければ飛ばす)

## 文法(現時点の案と決定事項)

決定事項:

- 関数は既定で非公開。JS から呼べる関数は `export fn` と書く。属性は `@wasm export fn` の順に付ける。従来の `pub` も互換性のために受け付ける。

- クロージャは `(x) => e` に一本化する
- 数値は `i32` / `u32` / `i64` / `f64` で、暗黙変換はしない(`Int` / `Float` の別名は置かない)
- class は持たない
- effect 多相は入れず、簡単な制約で済ませる(将来の拡張の余地は残す)
- 配列は TS の `readonly T[]` そのもの。JS の配列をコピーも変換もなしに渡せるので、コードサイズとブリッジの両面で有利。不変として扱い、`push` は新しい配列を作る
- 可変性はローカル変数に限る(Koka の `var` に近い)
  - `let mut` の変数への代入と `+=` などの複合代入、`while`、`for x in xs` / `for i in range(a, b)`、`loop`、`break` / `continue`、`return` を書ける
  - レコードと配列は不変のまま。可変のローカル変数はクロージャから捕捉できない
  - そのため関数は外から見て純粋なまま。JS ではただの `let` と `for` になり、Rust では `let mut` になるので、借用の問題も起きない
- `.d.ts` の数値は branded type にする

```rust
// データ: レコード、enum (代数的データ型)。class はなく、振る舞いは関数と match で書く
type Todo = { id: i32, title: String, done: bool }
enum Filter { All, Active, Done }          // データなし → "All" | "Active" | "Done"
enum Shape { Circle(f64), Square(f64) }    // データあり → { TAG: "Circle", _0: r } (TS の判別共用体)
// Option<T> / Result<T, E> は宣言せずに使える (下の「Option / Result と `?`」)

// 関数: トップレベルのシグネチャは注釈必須、本体は推論。let は不変
export fn visible(todos: Array<Todo>, f: Filter) -> Array<Todo> {
  todos |> .filter((t) => match f { All => true, Active => !t.done, Done => t.done })
}
fn toggle(t: Todo) -> Todo { Todo { done: !t.done, ..t } }   // 更新は新しい値を作る

// num-traits 相当: 数値の bound を持つ関数は型ごとに単相化される (sum$i32 / sum$f64 / ...)
export fn sum<T: Num>(xs: Array<T>) -> T { xs.fold(T::zero(), (acc, x) => acc + x) }

// JSX: コンポーネントは `fn Name(props: Record) -> Jsx`。props は型検査される
export fn Row(props: RowProps) -> Jsx { <li onClick={() => props.onPick(props.sample.label)}>{props.sample.label}</li> }

// JS の関数を手書きで宣言して取り込む (関数の型の自動取り込みはまだない)
extern "react" fn useState<T>(init: T) -> (T, fn(T) -> ()) = "useState";
// JS の値の型と、そのプロパティ・メソッド (下の「JS の値」)
extern type Url = "URL" { host: String, fn href() -> String = "toString"; }

// TS のファイルのレコード型を、その型定義のとおりに読み込む (下の「TS の型の取り込み」)
import type { User, Post } from "./model.ts";
```

### モジュール: `import { .. } from "./lib.zsc"`

別の `.zsc` の型と関数を使えます。各モジュールは TS と同じく 1 つずつコンパイルし、出力は同じディレクトリの木に置きます(`./lib.zsc` は `./lib.js` / `./lib.d.ts` になります)。

```zsc
// shapes.zsc
export type Point = { x: f64, y: f64 }
export enum Shape { Circle(Point, f64), Rect(Point, Point) }
export fn area(s: Shape) -> f64 { .. }
export fn Badge(props: Labeled) -> Jsx { .. }
```

```zsc
// app.zsc
import { Shape, area, Badge } from "./shapes.zsc";

export fn total(shapes: Array<Shape>) -> f64 { shapes.fold(0.0, (acc, s) => acc + area(s)) }
```

- 取り込めるもの
  - レコード型と enum。取り込んだ型が使う型(`Shape` の `Point`)も一緒に入る
  - `export fn`。コンポーネントも、`@wasm export fn` も(その JS の export を呼ぶ)
- 出力
  - JS は `import { area, Badge } from "./shapes.js";`。`.d.ts` は `import type { Shape } from "./shapes.js";` で、型を宣言し直さない(TS からは同じ型)
  - 取り込んだ関数は、そのモジュールの JS を指す `extern fn` と同じ扱い
- 取り込む側は、相手のモジュールを読んで型検査する。相手のエラーは相手のファイルの位置で報告する。循環した import はエラー
- まだできないこと
  - 数値の bound を持つ総称関数(数値の型ごとに別の JS 関数になる)、effect を perform する関数(JS では generator)、Rust の `wasm fn`
  - `@wasm fn` から、取り込んだ関数を呼ぶこと(この wasm の中にはない: JS の関数になる)。取り込んだ型は `@wasm fn` でも使える
  - `--emit-rust` での取り込み、パッケージ(`"lib"`)からの取り込み、プレイグラウンドでの複数ファイル

### TS の型の取り込み: `import type`

取り込めるのはレコード型、文字列リテラルの union、共通のタグで判別できる union です。たとえば、TypeScript で処理の状態を次のように定義します。

```ts
// model.ts
export type State = { kind: "idle" } | { kind: "busy"; jobs: number };
```

```zsc
import type { State } from "./model.ts";

export fn jobs(state: State) -> f64 {
  match state {
    Idle => 0.0,
    Busy(n) => n,
  }
}
```

JS から `jobs({ kind: "busy", jobs: 3 })` と呼ぶと `3` が返ります。タグだけの `idle` は括弧を付けず `Idle`、値を持つ `busy` は `Busy(n)` と書きます。複数の値を受け取る場合、括弧内の変数はタグを除いたフィールドの **TS での宣言順** に対応します。変数名は自分で付けられます。
たとえば `{ kind: "rect"; height: number; width: number }` の幅 ÷ 高さは `Rect(h, w) => w / h` です。TS 側を `width, height` の順に変えたら、パターンも `Rect(w, h)` に直します。両方とも `number` なので、元のパターンのままでもコンパイルは通り、計算結果だけが変わります。

```zsc
import type { User, Post } from "./model.ts";   // model.ts の interface / type を、レコード型として読む

export fn display(u: User) -> String { u.name + " #" + (u.id as i32).to_string() }
```

- **レコード型** (src/tsimport.rs): `interface X { .. }` か `type X = { .. }` で、型引数、`extends`、任意のフィールド(`?`)がないもの
  - フィールドの型: `string` は `String`、`number` は `f64`(JS の number は倍精度。整数にするときは `as i32`)、`boolean` は `bool`、`bigint` は `i64`。配列(`T[]`、`readonly T[]`、`Array<T>`、`ReadonlyArray<T>`)と、同じファイルの別のレコード型
  - 読み込む型が使う型(`Post` の `author: User`)も一緒に読む。export されていない型でもよい
  - ファイルの残り(関数、union、総称型など)は、import しない限り読まない。import した型が部分集合の外なら、TS のファイルのその位置でエラーになる(`` `Opt.name` is optional ``、tests/errors/ts_*.zsc)
  - パスは zsc のファイルからの相対パス。パッケージ(`"react"`)からはまだ読まない
- **union は enum として読む**(JS では TS の値そのまま)
  - 文字列リテラルの union `type Status = "draft" | "published"`: データのない enum。バリアントはタグの先頭を大文字にした名前(`Draft`、`Published`)で、JS の値は TS の文字列そのもの(`"draft"`)
  - 判別キーつきの union `type Shape = { kind: "circle"; r: number } | { kind: "rect"; w: number; h: number }`: 全メンバーが同じ文字列リテラルのフィールド(タグ)を持つもの。メンバーはファイルの別のオブジェクト型の名前でもよい(`Square | { kind: "empty" }`)。バリアントは `Circle(f64)`、`Rect(f64, f64)` で、フィールドは TS のメンバーの順。パターン `Circle(r)` は `s.kind === "circle"` と `s.r` になり、`Circle(2.0)` は `{ kind: "circle", r: 2.0 }` を作る。`==` もタグとメンバー名で比べる
  - 読まないもの(エラー): 文字列とオブジェクトが混ざった union、共通のタグがない union、名前にならないタグ(`"in-progress"`)、大文字にすると同じになるタグ
  - TS から読んだ union も `@wasm` との境界を越える(下の「バイト列で越える値」、TS の形のまま)
- 読み込んだ型は、zustscript のレコードと同じに使える(フィールドの読み出し、`{ ..u }` の更新、`==`、`@wasm` との境界)
- JS の出力には何も足さない(レコードは素のオブジェクト)。`.d.ts` は自分で interface を書かず、`import type { User } from "./model.js"` で TS の型そのものを使う。TS 側は自分の値をそのまま渡せる(tests/ts/consumer.ts を tsc で検査)
  - export されていない型が `.d.ts` に出るときだけ、TS と同じ形(数値は素の `number`)の interface を書く。TS は構造で型を比べるので、同じ型として扱われる

### 数値(AssemblyScript と同じ程度の表現力)

| 型 | JS での表現 | 演算 |
|---|---|---|
| `i32` | number | `(a + b) \| 0`、`Math.imul`、`/` と `%` は 0 除算で例外を投げ、結果を切り捨てる |
| `u32` | number | `>>> 0` で wrap する |
| `i64` | BigInt | `BigInt.asIntN(64, ..)`。2^53 を超えても正確 |
| `f32` | number | 演算のたびに `Math.fround`。+ - * / と sqrt は、f64 で計算して f32 に丸めると f32 の演算と完全に一致する。リテラルは、コンパイル時に f32 に丸めた値を書く |
| `f64` | number | そのまま |

- 小数リテラルは文脈の型になる(f32 か f64。決まらなければ f64)
- f32 と f64 も暗黙には変換しない(`x as f32`)

### 数値専用の配列: `F64Array` / `F32Array` / `I32Array`

```zsc
let w: F32Array = [0.5, -1.0, 2.0];          // 型の注釈があるリテラルは new Float32Array([..]) になる
let z = F64Array::zeros(n);                  // new Float64Array(n)
let p = F64Array::from(xs);                  // Array<f64> から明示的に変換する(暗黙の変換はない)
export fn dense(..) -> F32Array { range(0, m).map((r) => ..) }   // 期待する型が F32Array なら、range の map もそれを作る
```

- JS では typed array そのもの(`Float64Array` / `Float32Array` / `Int32Array`)。`.d.ts` でもその型になる
- 型で明示的に作る
  - 期待する型(`let` の注釈、引数、戻り値、レコードのフィールド)が typed array のところでは、配列リテラルと `range(..).map` がその typed array を作る
  - `Array<f64>` の変数は暗黙には変換されない。`F64Array::from(xs)` か `xs` を作り直す。変換すれば型エラーにならない(tests/errors/typed_from_array.zsc)
- メソッド
  - `len`、添字、`map`(同じ型の typed array を返す)、`filter`、`fold`、`any` / `all`、`for x in xs`、`to_array()` が使える
  - 不変として扱うので、`push` / `concat` はない
- 境界(`@wasm` / `@simd`)では、`.set()` でまとめてコピーし、結果は `.slice()` で取り出す。普通の配列のように、要素ごとの変換をしなくて済む

- `.d.ts` では branded type になる。TS 側でも整数と小数が混ざらない
  - 例: `export type i32 = number & { readonly __zsc_num: "i32" }`
  - brand は文字列リテラルのプロパティで付けるので、モジュールが違っても互換になる(`unique symbol` だとモジュールごとに別の型になる)
  - 値は number のままなので、`i32` は `number` に代入できる。逆向きは、モジュールが export するコンストラクタ `i32(x)` / `u32(x)` / `f64(x)` を通す
  - コンストラクタは変換せず検査だけする: `i32(1.5)` は RangeError。境界で混入したら誤りとして止める
  - `i64` は TS の `bigint` で、もともと number と区別されるので brand は付けない
  - tests/consumer.ts を tsc(`--strict`)にかけて、`@ts-expect-error` で次が型エラーになることを確かめている: f64 を i32 に渡す、素の number を i32 に渡す、readonly のフィールドや配列を変更する
- 整数リテラルは文脈の整数型になり、決まらなければ `i32` になる。`1.5` は `f64`
- `x + 1`(x が f64)は型エラーになる
- 変換は `as` だけ。f64 からの変換は Rust と同じく飽和する(`1e10 as i32 == 2147483647`)
- 数値の bound(`Num` / `Integer` / `Float`)付きの総称関数は、使われた型ごとに別の JS 関数として書き出す(演算の JS 表現が型ごとに違うので、型消去ではなく単相化する)
  - 公開した関数は、bound が許すすべての型の分を export する
  - `.d.ts` も型ごとに出す

### ローカルな可変性とループ

```zsc
export fn primes_below(n: i32) -> i32 {
  let mut count = 0;
  for k in range(2, n) {            // JS: for (let k = 2; k < n; k++)    Rust: for k in (2i32)..(n)
    let mut d = 2;
    let mut prime = true;
    while d * d <= k {
      if k % d == 0 { prime = false; break; }
      d += 1;                       // JS: d = (d + 1) | 0                Rust: d = d.wrapping_add(1)
    }
    if prime { count += 1; }
  }
  count
}
```

- `let mut` でない変数への代入、可変のローカル変数をクロージャが捕捉すること、ループの外の `break` はエラーになる
- ループの本体は `()` を返す
- `loop { .. }` は `break` か `return` で抜ける。`break` で抜けるループは `()`、`break` のないループは値を持たない(どの型としても使える)。`break value` はまだない

`return` は、いちばん内側の関数かクロージャから値を返します(Rust と同じ)。

```zsc
fn find_pos(xs: Array<i32>, v: i32) -> i32 {
  let mut i = 0;
  loop {
    if i >= xs.len() { return -1; }
    if xs[i] == v { return i; }
    i += 1;
  }
}

fn doubled(o: Option<i32>) -> i32 {
  let n = match o { Some(n) => n, None => return 0 };   // JS: if (o.TAG === "Some") { n = o._0; } else { return 0; }
  n * 2
}
```

- `return;` は `()` を返す。値の型は関数(クロージャ)の結果の型と同じでなければならない
- `{ ..; return x; }` のように `return`(か `break` / `continue`、`break` のない `loop`)で終わるブロックは、値を持たない。`if` の片方の枝や `match` の腕に書ける
- 式の中の `return` は、`?` と同じく、その式の前の文として書き出す(`f(g(), match o { .., None => return 0 })` は `g()` を先に評価してから返る)
- 書けない所: `handle` の中(本体と腕は JS では別の関数)、`&&` / `||` の右辺と `while` の条件(`?` と同じ)、`@wasm` / `--emit-rust` のクロージャの中

### ガイドライン: 数値の型と、どの層で書くか

**数値の型**

| 型 | 使いどころ | JS での扱い |
|---|---|---|
| `i32` | **整数の既定**(添字、個数、ID、整数の計算全般)。整数リテラルは、文脈がなければ i32 になる | number のまま(`\| 0` と `Math.imul`)。JIT が整数として扱えるので速い |
| `f64` | 実数 | number のまま |
| `f32` | ML の重みや画像など、精度より量と SIMD の幅(4 レーン)が効くもの。`F32Array` と組み合わせる | `Math.fround` が付く |
| `u32` | ハッシュ、ビット列、色など、符号なしの wrap が要るもの | number のまま(`>>> 0`) |
| `i64` | **JS 層では避ける**。2^31 を超える整数が本当に要る計算は `@wasm` に置く | BigInt になる(遅く、割り当てが起きる)。JS 層で i64 の演算を書くと、コンパイラが警告を出す |

**関数の実行場所と出力先**

| 書き方 | 向いているもの | 切り替え |
|---|---|---|
| 普通の `fn`(JS 層) | UI、データの整形、ほとんどのロジック。f64 / i32 のループも、V8 の JIT で十分に速い | - |
| `@simd fn` | 数値の配列に対する畳み込み(内積、合計)と要素ごとの計算。f64x2 / i32x4 で処理する | `--no-simd` で `@wasm` に、`--all-js` で JS に戻せる |
| `@wasm fn` | i64 を多用する計算、整数の wrap を多用する計算、レコードや enum を使う重い処理 | Rust 経由で実行。JS 専用出力と併用するとエラー |
| `wasm fn`(本体は Rust) | Rust のライブラリ的なコード、所有権やメモリを直接扱う処理 | - |

- 小さい関数を何度も呼ぶ用途には、`@wasm` / `@simd` を使わない。境界を越えるたびにコストがかかる
- 配列を 1 回渡して軽い処理をするだけの用途も同じで、コピーのほうが高くつく
- `@simd` の f64 の合計は、レーンごとに足してから最後にまとめるので、足す順番が変わる。そのため末尾の桁が JS と変わりうる。これを受け入れることが `@simd` を付ける意味になる。要素ごとの計算と i32 の合計は、JS と完全に一致する

### パイプライン `|>`

- 左辺を第1引数に入れる: `x |> f(a)` は `f(x, a)`、`x |> f` は `f(x)`
- メソッドにも使える: `xs |> .map(g)` は `xs.map(g)`

### 非同期: `async fn` / `await` / `Promise<T>`

JS の async / await をそのまま使います。

```zsc
extern fn fetchText(url: String) -> Promise<String> = "fetchText";
extern fn log(s: String) = "console.log";

export async fn title(id: i32) -> String {
  let body = await fetchText(`/items/${id}`);
  body.trim()
}

export fn App() -> Jsx {
  <button onClick={async () => { let t = await title(1); log(t); }}>load</button>
}
```

- `async fn f() -> T` を呼ぶと `Promise<T>` が返る。本体は `T` を返す。`.d.ts` は `Promise<T>`
- `await e` は `Promise<T>` の `e` から `T` を取る。書けるのは `async fn` と `async (..) =>` の中だけ(ふつうのクロージャの中は不可。JS と同じ)
  - `await f() + 1` は `(await f()) + 1`(前置の演算子と同じ結合)
  - JS で関数になる式(被演算子の位置の `match` など)の中の `await` は、その関数を async にして `await` する
- `Promise<T>` は JS の値の型。作るのは `extern` の関数(`fetch` などを包んだもの)と `async fn`
- まだできないこと
  - `@wasm` / `@simd` / `--emit-rust`(Promise は JS のもの)
  - effect を perform する async fn(effect は generator で動く)。effect は async でない関数の中で handle する
  - `Promise.all` などの組み合わせ(`extern` で受ける)
- reject される Promise は、`extern` の結果を `Promise<Result<T, String>>` と宣言して `Err` で受ける(下の「JS の値」)

### Option / Result と `?`

`Option` と `Result` は宣言せずに使えます(prelude)。

```zsc
enum FormError { Empty(String), TooLong(String, i32) }

fn non_empty(field: String, s: String) -> Result<String, FormError> {
  if s.trim().len() == 0 { Err(Empty(field)) } else { Ok(s.trim()) }
}

fn validate(d: Draft, id: i32) -> Result<Todo, FormError> {
  let title = non_empty("title", d.title)?;     // Err なら、その Err を validate の結果として返す
  Ok(Todo { id, title })
}

fn title_of(todos: Array<Todo>, id: i32) -> String {
  find(todos, id) |> .map((t) => t.title) |> .unwrap_or("(none)")
}

fn first(todos: Array<Todo>) -> String {
  if let Some(t) = find(todos, 1) { t.title } else { "-" }
}
```

- prelude: `enum Option<T> { None, Some(T) }` と `enum Result<T, E> { Ok(T), Err(E) }`。ふつうの enum と同じに扱う(JS は `{ TAG: "Some", _0: x }`、`.d.ts` は使うときだけ宣言する)
  - 同じ形の宣言はそのまま通る(prelude より前に書いたファイルのため)。違う形の `Option` / `Result` はエラー
  - 別の enum が `None` / `Some` / `Ok` / `Err` という名前のバリアントを持つファイルでは、その prelude の型は入らない(バリアント名はファイルの中で一意)
  - `None` / `Some` / `Ok` / `Err` という名前の関数はエラー
- `e?`
  - `Ok(x)` / `Some(x)` なら `x`
  - `Err` / `None` なら、それをいちばん内側の関数(クロージャの中ならクロージャ)の結果として返す
  - 型の規則
    - 関数の結果は、`Result` なら同じエラーの型の `Result`、`Option` なら `Option`。エラーの型の変換はしない(`.map_err(f)?` で揃える)
    - `Option` を返す関数で `Result` に付けるときは `.ok()?`、その逆は `.ok_or(e)?`
  - 書けない場所(エラー)
    - `&&` / `||` の右辺、`while` の条件
    - `handle` の本体と arm(クロージャの中は書ける)
  - 後置の演算子で、呼び出しや `.f` と同じ強さ
    - `x |> f(a)?` は `f(x, a)?`
    - `await f()?` は `await (f()?)` なので、Promise の結果には `(await f())?` と書く
  - 予約: `?.`(`(e?).f` か `e.map((x) => x.f)` と書く)、`??`(`.unwrap_or(d)`)、三項演算子 `a ? b : c`(`if`)
  - JS の出力は、文の前に書く `const $t = e; if ($t.TAG === "Err") return $t;`。`?` より左の被演算子は先に一時変数に置き、評価の順は書いたとおり
  - `@wasm` では Rust の match(`Err` / `None` を返す)
- `if let P = e { a } else { b }` は `match e { P => a, _ => b }`
- メソッド(名前は Rust と同じ)
  - `Option`: `map` `and_then` `filter` `unwrap_or` `unwrap_or_else` `ok_or` `ok_or_else` `is_some` `is_none`
  - `Result`: `map` `and_then` `map_err` `unwrap_or` `unwrap_or_else` `ok` `is_ok` `is_err`
  - 配列の `try_map(f)`: `f` が `Result` を返すなら `Result<Array<U>, E>`、`Option` なら `Option<Array<U>>`。最初の `Err` / `None` で止まる
  - JS では小さなヘルパー(`$o_map` など)。`@wasm` ではまだ使えない
- まだないもの: `unwrap()` / `expect()`、任意のフィールド `x?: T`(提案 003 の C)、`zsc-prelude.d.ts`(今は `.d.ts` ごとに `Option` / `Result` を宣言する)

### JS の値: `extern type`、`null`、例外

JS の値を、中を見ない型として宣言します。プロパティとメソッドも宣言できます。

```zsc
extern type Response {
  ok: bool,
  status: i32,
  fn text() -> Promise<String>;
}
extern fn fetch(url: String) -> Promise<Result<Response, String>> = "fetch";   // reject は Err(メッセージ)

extern type Params = "URLSearchParams" { fn get(name: String) -> Option<String>; }   // null は None
extern type Url = "URL" { host: String, params: Params = "searchParams" }
extern fn parse_url(s: String) -> Result<Url, String> = "new URL";              // throw は Err(メッセージ)

extern type Json = "unknown";
extern fn parse_json(s: String) -> Result<Json, String> = "JSON.parse";

export async fn load(url: String) -> Result<String, String> {
  let r = (await fetch(url))?;
  if r.ok { Ok(await r.text()) } else { Err(`status ${r.status}`) }
}
```

- `extern type Name;`: JS の値の型
  - `= "TsName"` で TS での名前を付ける(既定は同じ名前)。`.d.ts` ではその名前を使う
  - `extern "module" type Name;` は、そのモジュールの型(`.d.ts` は `import type`)
  - TS に型のない値は `= "unknown"` にする
- メンバー
  - プロパティ: `host: String`(JS の名前が違うなら `path: String = "pathname"`)。`u.host` で読む
  - メソッド: `fn get(name: String) -> Option<String>;`(`= "jsName"` も同じ)。`p.get("k")` で呼ぶ
  - 代入: `fn set_label(s: String) = "label=";`(JS の名前が `=` で終わる)
- `extern fn` の JS の名前が `new X` なら、コンストラクタの呼び出し
- `extern` の関数とメンバーの境界での変換
  - 結果の `Option<T>`: `null` / `undefined` は `None`、ほかは `Some`
  - 引数の `Option<T>`: `None` は `undefined`
  - 結果の `Result<T, String>`: JS が throw したら `Err(メッセージ)`。エラーの型を extern type にすると、throw された値そのもの(`extern type JsError = "Error" { message: String }`)。ほかのエラーの型はエラー
  - 結果の `Promise<Result<T, E>>`: reject も `Err` になる
  - 変換は、宣言に書いた型で決まる。型引数(`extern type Box<T> { value: T }` の `T`)に `Option` が入っても変換しない。JS の側は zustscript の値をそのまま持つため(`Box<Option<i32>>` に入れた `Some(1)` は `{ TAG: "Some", _0: 1 }` のまま返る)
  - 例外を受ける呼び出しでは、引数は呼び出しの外で先に評価する(引数の `await` は囲む async 関数のもの)
- `--check-externs` は、メンバーも TS の型と照合する(`Option` の結果は `T | null | undefined`、`Result` の結果は `T` として)
- まだないもの: `@wasm` での extern type、`==`(同じ値かどうか)、型引数を持つメソッド、TS の `.d.ts` からの自動の取り込み

### 標準ライブラリ: 配列・文字列・数値のメソッド

名前と意味は Rust に合わせ、JS と `@wasm` で同じ結果にします。文字列の位置と長さは JS と同じ UTF-16 の単位です(`tests/stdlib.zsc` で JS と wasm の結果を比べている)。

| 対象 | メソッド |
| --- | --- |
| 配列 | `len` `map` `filter` `fold` `any` `all` `push` `concat` `join` `try_map`、`get(i)` / `first()` / `last()`(`Option`)、`is_empty`、`slice(a, b)`、`reverse`、`includes(x)` / `index_of(x)`(`==` と同じく中身で比べる)、`find(f)` / `find_index(f)`、`flat_map(f)`、`sort()`(数値か文字列)、`sort_by((a, b) => ..)`(負・0・正の i32 を返す。安定) |
| 文字列 | `len` `to_upper` `to_lower` `trim` `trim_start` `trim_end` `contains` `starts_with` `ends_with` `split`、`index_of(s)`(`Option<i32>`)、`slice(a, b)`、`replace(from, to)`(すべて置き換える)、`repeat(n)`、`chars()`(コードポイントごとの `Array<String>`)、`is_empty`、`to_i32()` / `to_f64()`(`Option`) |
| f32 / f64 | `floor` `ceil` `round` `trunc` `sqrt` `abs` `max` `min`、`pow(y)`、`exp` `ln` `log10` `log2`、`sin` `cos` `tan` `asin` `acos` `atan` `atan2(x)`、`is_nan` |
| i32 / u32 / i64 | `abs`(u32 以外)、`min(y)` `max(y)`、`pow(e)`(`e: i32`。桁あふれは折り返し、負の `e` はエラー) |

```zsc
fn top(names: Array<String>) -> String {
  let sorted = names.filter((n) => !n.is_empty()).sort();
  match sorted.first() { Some(n) => n.to_lower(), None => "-" }
}
fn parse_all(xs: Array<String>) -> Option<Array<i32>> { xs.try_map((x) => x.trim().to_i32()) }
```

- `slice(a, b)` は JS の `slice` と同じ: 負の位置は末尾から数え、範囲は長さに切り詰める
- `to_i32` / `to_f64` は Rust の `parse` と同じ: 符号と数字だけ(空白や `0x` は `None`)。小数は `.5`、`1e3`、`inf`、`NaN` も読む
- `sort()` の NaN は、どの数とも等しい扱い(JS の比較と同じ)。NaN を含む配列の並びは決まらない
- `@wasm` の数学関数(`pow` `sin` など)は、グルーが JS の `Math` を zustc に渡すので、JS 層とビット単位で同じ。`--emit-rust` では Rust の std(libm)の結果になり、最後の桁が違うことがある
- 分かっている違い: サロゲートペアの途中で切る文字列の `slice` は、JS では孤立したサロゲート、wasm では U+FFFD になる
- まだないもの: 乱数、`enumerate`

### パターン: `|`、ガード、レコード

```zsc
fn kind(t: Tok) -> i32 {
  match t {
    Plus | Minus => 1,
    Num(n) if n < 0 => -1,          // ガード: パターンの名前が使える
    Num(_) => 2,
    _ => 0,
  }
}
fn x_of(e: Ev) -> i32 { match e { Click(x, _) | Move(_, x, _) => x, Key(_) => -1 } }
fn quadrant(p: Point) -> i32 {
  match p { Point { x: 0, y: 0 } => 0, Point { x, y } if x > 0 && y > 0 => 1, _ => 2 }
}
fn greet(u: User) -> String { let User { name, .. } = u; name }
```

- `A | B` は入れ子にも書ける(`Some(Circle(_) | Square(_))`)。どの選択肢も同じ名前を同じ型で束縛する
- ガード `pat if cond =>` は、パターンの名前を使える(`?` も書ける)。ガードのある腕は網羅性の検査では数えない
- レコードのパターン `Point { x, y: py, .. }` は `match` / `if let` / `let` に書ける。書かないフィールドがあれば `..` で終える。`let` には `|` を書けない(`match` で書く)
- JS
  - `|` は 1 つの条件(`e.TAG === "A" || e.TAG === "B"`)。束縛は、合った選択肢でのその位置
  - ガードのある match はラベル付きのブロックで、腕ごとの `if` が合えばそこから抜ける
  - `let` の分解は JS の分割代入(`const { name, age } = u;`)
- Rust では Rust の `|`、ガード、構造体のパターン(`@wasm` での `let` の分解はまだない)
- まだないもの: バリアントの `..`(`Rect(..)`)、クロージャの引数と `for` の分解

### `impl`: メソッドと関連関数

```zsc
type V2 = { x: f64, y: f64 }
impl V2 {
  fn new(x: f64, y: f64) -> Self { Self { x, y } }       // V2::new(1.0, 2.0)
  fn add(self, o: V2) -> V2 { V2 { x: self.x + o.x, y: self.y + o.y } }
  fn len(self) -> f64 { (self.x * self.x + self.y * self.y).sqrt() }
}
impl<T> Stack<T> {
  fn push(self, x: T) -> Stack<T> { Stack { items: self.items.push(x) } }
}
impl<T: Eq> Stack<T> {
  fn has(self, x: T) -> bool { self.items.includes(x) }
}

fn demo() -> f64 { V2::new(3.0, 4.0).add(V2::new(0.0, 0.0)).len() }   // JS: V2$len(V2$add(V2$new(3, 4), ..))
```

- レコードにも enum にも書ける。最初の引数が `self`(型は書かない)の関数はメソッド `v.len()`、それ以外は `V2::new()` で呼ぶ。`V2::len(v)` や `vs.map(V2::len)` のように関数としても使える
- `Self` はその型(型、`Self { .. }`、`Self::new()`、パターン)
- 中身はトップレベルの関数 `V2$len`(引数の最初が受け手)で、JS でも Rust でも普通の呼び出しになる。`impl<T>` の型引数は関数の型引数になる(bound も)
- まだできないこと
  - `export`(TS から呼ぶ名前を決めていない: 呼び出す関数を `export` する)
  - effect を perform するメソッドのメソッド呼び出し(`P::f(x)` と関数として呼ぶ)
  - 他のモジュールから取り込んだ型のメソッド、トレイト

### 総称関数の bound: `Eq` / `Ord` / `Hash`

```zsc
fn dedupe<T: Eq>(xs: Array<T>) -> Array<T> {
  xs.fold([], (acc: Array<T>, x) => if acc.includes(x) { acc } else { acc.push(x) })
}
fn max_of<T: Ord>(xs: Array<T>, d: T) -> T { xs.fold(d, (m, x) => if x > m { x } else { m }) }
fn uniq<T: Hash>(xs: Array<T>) -> Array<T> { Set::from(xs).values() }
```

- `T: Eq` で `==`(中身で比べる。関数や JSX は渡せない)、`T: Ord` で `<`(数値と文字列)、`T: Hash` で `Map` のキー・`Set` の要素(整数、文字列、bool、データのない enum)。数値の bound(`Num` など)はどれも `Eq` を、`Integer` は `Hash` も満たす
- 数値の bound と同じく、使われた型ごとに別の関数として書き出す(JS の `dedupe$P`、Rust の `zs_dedupe__P`)
- `export fn` には付けられない(型を列挙できない): 使う型ごとの関数を `export` する

### `Map` / `Set`

```zsc
fn words(s: String) -> Map<String, i32> {
  let mut counts: Map<String, i32> = Map::new();
  for w in s.split(" ") {
    counts = counts.insert(w, counts.get(w).unwrap_or(0) + 1);   // JS: counts = counts.set(w, ..)(その場で)
  }
  counts
}
```

- 生成: `Map::new()`、`Map::from([(k, v), ..])`、`Set::new()`、`Set::from(xs)`
- `Map`: `len` `is_empty` `get(k)`(`Option`) `contains_key(k)` `insert(k, v)` `remove(k)` `keys()` `values()` `entries()`(`Array<(K, V)>`)。`Set`: `len` `is_empty` `contains(x)` `insert(x)` `remove(x)` `values()`
- 配列と同じく不変: `insert` / `remove` は新しい値を返す。ほかから見えないローカル変数なら、その場で変える(`m = m.insert(k, f(m.get(k)))` の形も)
- 順序は JS の Map / Set と同じ挿入順(既にあるキーを入れ直しても位置は変わらない)。JS では JS の `Map` / `Set` そのもので、`.d.ts` は `ReadonlyMap` / `ReadonlySet`
- `@wasm` では、挿入順の配列と `HashMap` の索引を持つ型(`ZsMap` / `ZsSet`)。JS と同じ結果になる(`tests/collections.zsc`)。`Map::from` と `entries()`(タプル)、境界を越えることはまだできない
- `==` では比べられない。`Map` / `Set` という名前の型は宣言できない(組み込みの名前)。`Map::new` などは呼び出す形でだけ書ける(値として渡すならクロージャで包む。`json_encode` / `range` も同じ)

### JSON: `json_decode` / `json_encode`

```zsc
type Todo = { id: i32, title: String, due: Option<String> }

fn load(s: String) -> Result<Array<Todo>, String> { json_decode(s) }   // 型どおりか確かめて読む
fn save(ts: Array<Todo>) -> String { json_encode(ts) }
```

- `json_decode(s)` は、JSON が型どおりかを確かめて読む。違えば `Err("expected an i32 at $.todos[0].id, found 1.5")` のように場所を付けて返す(構文の誤りは JSON.parse のメッセージ)
- 型は文脈から決まる(決まらなければ注釈を求める)。型ごとに確かめる関数を書き出す
- JSON での形
  - 数値(i32 / u32 は整数で範囲内のもの)、文字列、bool、配列、タプル(配列)
  - レコードはオブジェクト(宣言にないフィールドは読み捨てる)
  - `Option` は `null` か値(フィールドがなければ `None`)
  - データのない enum は文字列、データのある enum は JS での値(`{ "TAG": "Circle", "_0": 1.5 }`、TS から読んだ union はその形)
  - `Map<String, V>` はオブジェクト(他のキーは `[key, value]` の配列)、`Set` は配列
- 使えない型: i64(JSON.parse が 2^53 を超える整数を丸める)、`Option<Option<T>>`(`None` と `Some(None)` がどちらも `null` になる)、関数、JSX、`extern type`、総称の型引数
- JS 層だけ(`@wasm` ではエラー)

### 代数的エフェクト(enum で宣言する)

```rust
enum Console { Print(String) -> (), Ask(String) -> String }   // 全バリアントが `Op(..) -> R` の enum が effect

fn greet() -> String {                    // effect の注釈は不要: 呼び出しグラフ上の不動点で推論する
  let name = perform Ask("name?");
  perform Print("hello, " + name);
  name
}
let who = handle greet() {
  Print(s), k => { log(s); k(()) },       // k は one-shot の継続。呼ばなければ中断になる (例外に相当)
  Ask(q), k => k("taro"),
};
```

- JS では、effect の handler がすべて tail-resumptive(arm が末尾で `k(x)` を呼ぶだけ)なら、普通の関数呼び出しになる
  - `perform` は `$perform("Op", ..)` で、最も内側の handler の arm を直接呼ぶ。`k(x)` は `x` を返すことになる。arm は、その `handle` の外側の handler の下で動く(深いハンドラの意味のまま)
  - それ以外の effect(arm が `k` を呼ばずに抜けるもの、公開関数から外へ出るもの、それらと混ざるもの)では、関数は `function*`、`perform` は `yield`、その呼び出しは `yield*` になる。`handle` は深いハンドラ(一度だけ再開できる継続)で、generator は `$handle(..)` / `$resume(k, x)` を命令として `yield` する
  - 命令は 1 つのループ `$run` が実行する。フレーム(generator)はヒープ上のスタックに積み、operation はその handler から上のフレームを切り取って継続にし、再開はそれを積み直す。そのため、1 つの `handle` の中で operation を何回しても JS のスタックは深くならない(回帰テスト `gen_after_k` / `gen_outer_op` / `gen_plus_one`、10 万回)
  - 公開関数の generator は、本体をこのループ(`$drive`)の下で動かす。TS の呼び出し元に届くのは、その関数が扱わずに外へ出す operation だけで、`.next(x)` の値がその operation の結果になる(ループの命令は見えない)
  - arm が最後に `k(x)` を返すとき(`$tail`)は、arm のフレームを捨ててから再開するので、ヒープのスタックも伸びない。`k(()) + 1` のように再開の後に続きがあるときは、待っている arm の分だけヒープに積まれる
  - 直接の呼び出しは generator の約 100 倍速い(`bench/js` の `w_effects`)。generator の handler は、以前の `yield*` を入れ子にする実装より 4〜7 倍速い(operation 1000 回で 1.2〜1.4 ms → 0.2〜0.3 ms)
  - 扱わない effect は外側のハンドラへ転送される
  - handler の arm の中で別の effect を `perform` することもできる
- 制約(effect 多相の代わりに置いた簡単な規則)
  - effect は値を通らない: 未処理の effect を持つクロージャ、effect を持つ関数を値として使うこと、クロージャの中での `k` の呼び出しはエラー
  - handler は 1 つの effect の操作をすべて受け持つ
  - effect の enum は型引数を持たない
  - コンポーネント(`-> Jsx`)は effect を持てない(描画は中断できない)
- 将来の拡張: 関数型に effect 行 `fn(A) -> B / e` を足せば、`map` などに effect を通せる
  - 推論の不動点はそのまま使える
  - 生成側は「effect を持つクロージャはジェネレータ」「`map` は effect 付きの版」を足す形になる
  - 状態を `k` に通すスタイル(`(s) => k(x)(s)`)もここで書けるようになる

### wasm 層(Rust そのまま)と境界

```rust
export wasm fn stddev(xs: &[f64]) -> f64 { ... }                       // 本体は Rust。zustc がコンパイルする
export wasm fn map_sum(xs: &[f64], f: &dyn Fn(f64) -> f64) -> f64 { ... }  // 借用コールバック
export wasm fn call_twice(f: Box<dyn Fn(i32) -> i32>, x: i32) -> i32 { ... } // 所有コールバック
wasm { fn sq(x: f64) -> f64 { x * x } }                              // wasm 側だけの補助
```

境界の扱いはシグネチャの Rust の型だけで決まる。wasm_bindgen のマクロや注釈は不要。

| Rust 側 | JS 層 | 渡し方 | 誰が解放するか |
|---|---|---|---|
| i32 u32 f64 bool | 同じ型 | wasm の値 | なし |
| i64 | i64 (BigInt) | wasm の i64 | なし |
| `&[f64]` / `&[i32]` | `Array<f64>` / `Array<i32>` | 呼び出しの間だけ線形メモリへコピー | グルーが呼び出し後に解放 |
| `&str` | String | UTF-16 → UTF-8 に変換してコピー | グルーが呼び出し後に解放 |
| `String` / `Vec<..>`(戻り値) | String / Array | wrapper が (ptr, len, cap) を書き、グルーがコピーで受け取る | グルーがコピーの後に解放 |
| `&dyn Fn(A) -> R` | `(A) => R` | 表に登録した id | **呼び出しが返ったらグルーが解除**(借用なので呼び出しより長く生きられない) |
| `Box<dyn Fn(A) -> R>` | `(A) => R` | 表に登録した id | **Rust が Box を drop した時点で解除**(`Drop` が import の `__zsc_cb_drop` を呼ぶ) |

- コールバックの寿命の推論は、Rust の型(借用か所有か)をそのまま寿命の宣言として読む
  - wasm_bindgen では `Closure::forget` や `Closure` の保持を手で書く必要があるが、ここではそれが要らない
  - 制約: 引数と戻り値はスカラーだけ
- テストで確かめていること: 借用・所有の両方とも、呼び出しの後で JS 側の表は空になる

### `@wasm fn` で Rust 経由の Wasm 実行を指定する

```zsc
@wasm export fn dot(a: Array<f64>, b: Array<f64>) -> f64 {
  range(0, a.len()).fold(0.0, (s, i) => s + a[i] * b[i])
}
@wasm export fn total<T: Num>(xs: Array<T>) -> T { xs.fold(T::zero(), (acc, x) => acc + x) }
```

- 本体は普通の zustscript で、JS 層と同じ検査器で型を検査する。そのうえで `lower.rs` が、型の付いた構文木から Rust のソースへ落とし、zustc が wasm にする。境界のグルーは `wasm fn` と同じ仕組みで生成する
  - 例: `dot` は `(0i32..(a.len() as i32)).fold(0.0f64, |s, i| s + a[i as usize] * b[i as usize])` になる
- JS 版と比較するときは、同じ本体から `@wasm` を外して `--all-js` でコンパイルする。テスト `numeric_same_in_js_and_wasm` で、両方の出力が一致することを確かめている(差分テスト)。比べている結果は次のとおり
  - i32 / u32 の wrap
  - 2^53 を超える i64
  - f64 から整数への `as` の飽和
  - 再帰
  - 配列の範囲外アクセスがエラーになること
- 意味をそろえる方法
  - JS 層の i32 / u32 / i64 の演算は、Rust では `wrapping_add` / `wrapping_div` などにする(0 除算は JS でも Rust でもエラー)
  - `as` は Rust 自身が飽和するので、そのまま使える
  - 添字 `xs[i]` は、JS では範囲を検査する `$at`、Rust では `xs[i as usize]`(負の添字は巨大な usize になり、範囲外として止まる)
- 部分集合(外れると、その構文の名前を出してエラーにする)
  - 型: 数値、bool、配列、それらを組み合わせたレコードと enum(入れ子、総称の `Option<T>` なども可)
    - スカラーだけのレコードと enum は、Rust では `#[derive(Clone, Copy, PartialEq)]` の struct / enum(`ZsVec2`、`ZsShape`)になり、値で渡す
    - 配列と、配列を含むレコード / enum は Copy にならない。借用、move、clone は zsc が推論する(次の「所有権の推論」)
    - JS との境界を越えられるのは、数値の配列、スカラーだけのレコード / enum、そして文字列や配列を含むレコードと、レコード・文字列・配列の配列(次の「バイト列で越える値」)
  - 構文: `let` / `let mut` と代入、`while` / `for` / `break` / `continue`、`if`、`match`(数値、bool、enum のバリアント、入れ子のパターン)、レコードの生成・フィールド・`..base` での更新、`as`、`T::zero()` / `T::one()`
  - 配列の操作: `len` `map` `filter` `fold` `any` `all` `push` `concat`、添字、`range(a, b)`(Rust では配列を作らずイテレータ `a..b` になる)、上の「標準ライブラリ」のメソッド
  - 呼び出し: Rust に変換可能な通常の補助関数、他の `@wasm` 関数、`wasm fn`、再帰
  - 文字列: 引数は `&str`、戻り値は `String`。`+`、`==` と大小比較、`len` `to_upper` `trim` `contains` `split` と上の「標準ライブラリ」のメソッド、配列の `join`、整数と bool の `to_string`(次の「文字列と `==`」)
  - 使えないもの: タプル、effect、値としての関数、文字列のパターン、浮動小数点数の `to_string`(JS と Rust で書式が違う)
- `T: Num` の関数は、wasm でもインスタンスごとに単相化する(`zs_total__f64`、`zs_total__i64` など)
  - JS から呼ばれるインスタンスだけに wrapper とグルーを付ける
  - 他の `@wasm` 関数からだけ呼ばれるもの(例の `step`)は、ただの Rust 関数になる
- 境界を越えられる配列は、i32 / u32 / i64 / f64 のもの(`Int32Array` / `Uint32Array` / `BigInt64Array` / `Float64Array` でコピーする)
- JS 側で生のバイト列を扱う試作は `zustscript-helper/bytes` から読み込める。`Uint8Array` や `DataView` の部分ビューをそのまま扱う。手書き `wasm fn` の `&[u8]` / `Vec<u8>` への接続は実装予定。[所有権とビルドサイズの実測](docs/target-spec.md#バイト列を-js-と共有する)
- レコードと enum は、境界を**スカラーのスロットに分解して**越える。線形メモリへのコピーも、それを解放する処理も要らない
  - 引数: フィールドごとに wasm の引数にする。`p: Vec2` は `p__x: f64, p__y: f64` になる
    - enum は、i32 のタグと、全バリアントのフィールドのスロットになる。選ばれていないバリアントの分は 0 で埋める
    - Rust 側の wrapper が値に組み立て直す
  - 戻り値: wrapper が glue の out 領域に、1 スロット 8 byte で書き込む。glue はそれを `DataView` で読み、`{ TAG: "Some", _0: { .. } }` を組み立てる
  - 例: `nearest(xs, ys, p: Vec2, mode: Mode) -> Option<Hit>`
- **バイト列で越える値**: 文字列や配列を含むレコード(`{ name: String, tags: Array<String> }`)、レコード・文字列・配列の配列(`Array<Person>`、`Array<Array<f64>>`)は、1 つの固定した形式のバイト列にして越える
  - 両側の補助: JS 側は npm のパッケージ **zustscript-helper**(helper/ts、`encode` / `decode` だけの約 100 行)、Rust 側は crate **zustscript_helper**(helper/rust、trait `Wire` と `to_bytes` / `from_bytes`)。zustc は 1 つのファイルをコンパイルするので、zsc は crate のソースを `mod zustscript_helper` として埋め込み、レコードごとの `impl Wire` を書く
  - 形式(第 1 版、helper/rust/src/lib.rs に表): little-endian、詰め物なし、型のタグなし(両側が型を知っている)。数値は 4 / 8 byte、bool は 1 byte、文字列は UTF-8 の長さ(u32)と UTF-8、配列は長さ(u32)と要素、レコードは宣言順のフィールド、enum はバリアントの番号(u32)とそのフィールド。両側の単体テストで、同じ値が同じバイト列になることを確かめている
  - glue は引数を `encode` して線形メモリに置き、Rust の wrapper が `from_bytes` で値に戻して参照で渡す。結果は `to_bytes` して返し、glue が `decode` する。JS の出力は `import { encode, decode } from "zustscript-helper"` を含むので、使う側はこのパッケージを入れる
  - 載る型: 数値、bool、文字列、配列、レコード、enum。スカラーだけのレコードと enum は、これまでどおりスロットで越える(速い)。スロットに収まらないもの(文字列を持つ enum など)と、TS から読んだ union がこの形式になる
    - enum の JS の形はスキーマが持つ(`["enum", key, variants]`): zustscript の enum は `{ TAG, _0 }`、データのない enum は文字列、TS の union は TS の形(`{ kind: "circle", r }`、`"draft"`)。Rust 側は zsc が `impl Wire` を書く
  - typed array と関数は、まだこの形式に載せない(tests/errors/wasm_wire_typed.zsc)
  - 差分テスト `wire_same_in_js_and_wasm`(tests/wire.zsc): 文字列と配列を含むレコード、レコードの配列、入れ子の配列、i64 の結果が、JS と wasm で一致する。TS から読み込んだレコード(`User`)もそのまま越える(`ts_records` の `badge`)
- 差分テスト `shapes_same_in_js_and_wasm`: examples/shapes.zsc で、レコード、enum、総称の `Option<T>`、ループ、可変のローカル変数の結果が、JS と wasm で一致することを確かめている

同じソースを JS と wasm で走らせた速さ(node 22、examples/numeric.zsc、`tests/numeric_bench.mjs`)。両方の結果は一致している。

| 処理 | wasm | JS (`--all-js`) |
|---|---|---|
| `dot`、要素数 1e6 の f64(呼び出しごとに 2 本の配列を線形メモリへコピー) | 27 ms | 44 ms |
| `total$f64`、1e6(足すだけ。コピーが支配的) | 20 ms | 14 ms |
| mandelbrot 160×120、反復の上限 200(点ごとに境界を 1 回越える) | 8.1 ms | 11.0 ms |
| `lcg`、u32 で 1e6 ステップ | 11.5 ms | 44.6 ms |
| `collatz_steps`、1..20000(i64。JS では BigInt) | 20.7 ms | 45.1 ms |

- 判断の目安: 配列を渡して軽い処理を 1 回するだけならコピーで負ける。計算が重い処理、整数の wrap、i64(BigInt は遅い)は wasm が 1.4〜3.9 倍速い
- これは zustc の今の wasm 出力(振り分け、アドレス検査あり)での数字
  - `--wasm-O` を付けてもほぼ同じで、サイズは 46 KB → 69 KB に増える
  - 後述のメモリモデルの改善が入れば、差はさらに開く見込み
- JS 側にも改善の余地がある。`range(..).fold` は今、配列を作ってから畳み込んでいるので、`for` ループにすれば JS も速くなる

ループを入れた後の計測(examples/shapes.zsc。JS 側もネイティブの `for` になった)。

| 処理 | wasm | JS |
|---|---|---|
| `collatz_max(300000)`(i64 の while ループ。JS では BigInt) | 242 ms | 465 ms |
| `nearest`、2e5 点、f64 のループ(2 本の配列をコピー) | 5.9 ms | 1.2 ms |
| `grow` を 1e5 回呼ぶ(呼び出しごとに enum とレコードが境界を往復) | 14.6 ms | 1.2 ms |

### 所有権の推論(Rust のライフタイム推論に当たるもの)

`@wasm` の本体は、`&`、`'a`、`.clone()` を書かずに、Copy でない値(配列、配列を含むレコード / enum)を扱える。zustscript の値は不変なので、共有は言語の上では常に安全で、推論は「それを Rust でどう書けば一番コピーが少ないか」を決める(`src/own.rs`、`src/lower.rs` の "Ownership")。

```zsc
type Poly = { pts: Array<f64>, weight: f64 }
@wasm fn total(p: Poly) -> f64 { .. }                                    // fn zs_total(p: &ZsPoly) -> f64
@wasm fn heavier(a: Poly, b: Poly) -> Poly { if total(a) >= total(b) { a } else { b } }
                                       // fn zs_heavier<'a>(a: &'a ZsPoly, b: &'a ZsPoly) -> &'a ZsPoly
@wasm fn first_pts(ps: Array<Poly>) -> Array<f64> { ps[0].pts }         // fn zs_first_pts<'a>(ps: &'a [ZsPoly]) -> &'a [f64]
@wasm fn reweigh(p: Poly, w: f64) -> Poly { Poly { weight: w, ..p } }   // fn zs_reweigh(p: ZsPoly, w: f64) -> ZsPoly
```

- **引数のモード**: 読むだけの引数は借用(`&[T]` / `&ZsX`)。丸ごと消費する(返す、フィールドに入れる、所有する引数に渡す、`push` する)引数は所有(`Vec<T>` / `ZsX`)
- **借用の結果**: 結果がいつも借用した引数(またはその一部)なら、ライフタイムつきの借用で返す。呼び出し側は借用のまま使い、消費するときだけコピーする
- **関数をまたいだ推論**: 引数のモードと結果の形は、呼び出しグラフ全体の不動点で決める
  - すべて「借用」から始める。lowering の途中で「コピーが要った」と分かったもの(消費された引数、引数の借用でない結果)を所有にして、その関数と呼び出し元だけを lowering し直す(worklist)
  - 変化は所有の方向にしか起きないので必ず止まる。各関数を lowering する回数は、(1 + 自分と callee の変化の数)回まで
- **move**: 関数が所有する値は、最後の使用で move し、それ以外では clone する
  - 最後の使用は、構造化された本体の上の後ろ向きの生存解析で求める。ループは不動点まで回し、名前ではなく束縛ごとに解く(shadowing でも正確)
  - これにより、`acc = acc.push(x)` と `xs.fold([], (a, x) => a.push(x))` はその場で push する(配列のコピーではなく、償却 O(1))
  - 使い終わる配列の `map` / `filter` / `for` は、要素を clone せずに取り出す(`into_iter`)
- **ローカルな借用**: 借用した引数の一部を `let` すると、借用のままになる(`let pts = &p.pts[..]`)
  - 所有するローカル変数の借用も、ブロックの残りがその変数を代入しなければ借用のままにする(`let firsts = zs_first_pts(&ps[..]);`)。その変数はブロックの終わりまで凍結され、move されず clone される。ブロックやアームの外へ出る借用と、結果として返る借用は所有にする
  - 生き続ける値の `match` は参照で束縛する(`match *s { ZsS::Poly(ref p, n) => .. }`)。生き続ける配列の `for` は `.iter()` で回す
- **安全側に倒すところ**: borrow checker と食い違いうる箇所では move しない
  - クロージャが捕捉した変数
  - 1 つの呼び出しの 2 つの引数に現れる変数(`f(&xs, g(xs))`)
  - 参照で回している `match` / `for` の対象。本体がそれを代入するなら、先にコピーしてから回す
- 生成した Rust は zustc の borrow checker がもう一度検査する。推論の誤りはコンパイルエラーになり、未定義動作にはならない
- JS との境界の wrapper は、これまでどおりスライスを渡して、所有された結果を受け取る。推論した署名がそれと違う関数(配列の引数を所有する、結果を借用で返す)には、間に小さな adapter を置く

測ったこと(`examples/owned.zsc`、`tests/owned_cases.zsc`。どちらも JS と wasm の結果が一致し、zustc の borrow checker を通る)。

| | wasm、推論あり(その場で push) | wasm、毎回コピー(これまでの出力) | JS、その場で push | JS、毎回コピー(これまでの出力) |
|---|---:|---:|---:|---:|
| `ramp_sum(10000)`(1 つずつ push して足す) | 2.4 ms | 633 ms | 1.7 ms | 76 ms |
| `ramp_sum(100000)` | **10.4 ms** | 59.8 s | **8.2 ms** | 50.9 s |

**JS 層の push もその場でする**(`own::inplace_pushes`)。JS の配列は参照で共有されるので、条件は「ほかの誰も見ていない配列」であること

- 対象: `let mut` の配列で、与えられる値がいつも新しい配列(リテラル、`range(..)`、`map` / `filter` / `push` / `concat` の結果)。引数や呼び出しの結果は、ほかの誰かが持っているかもしれないので外す
- 使い方: 読むだけ(`len`、添字、`for`、`map` などの受け手、演算子)か、最後の使用(その後は死んでいるので、どこへ渡してもよい)
  - 可変のローカル変数はクロージャから捕捉できないので、クロージャからも見えない
- `for` の走査対象が読む変数を本体が代入するなら、外す。JS の `for .. of` は配列そのものを回るので、その場で足すと新しい要素まで回ってしまう
- その場の push は `(acc.push(x), acc)`、concat は `$append(acc, ys)` になる。それ以外は、これまでどおり `[...acc, x]`
- テスト `js_push_in_place`(`tests/js_inplace.zsc`): 別名、引数、走査しながらの追加、レコードへの格納、関数に渡して戻ってきた配列、`&&` の右辺で、結果がコピーする push と同じになること
- `bench/js/workloads.zsc` では、配列を 1,000 回 push して作る `w_records` が 5.9 → 5.2 ms、`w_enums` が 11.8 → 11.3 ms(結果は同じ。ほかのモジュールの出力は変わらない)

- 推論の費用: 各関数を lowering する回数は、(1 + 自分と callee の署名の変化の数)回まで。一般の線形の上限はない(多くの callee の変化が重なる形もある)
  - まだ lowering していない関数を先に済ませてから、lowering し直す関数をまとめて処理するので、callee が多い関数でも、し直しはまとめて 1 回で済む
  - 測った例(400 段の呼び出しの鎖を、所有のモードが 1 段ずつ伝わる)では、関数の数に線形で、関数 1 つあたり約 29 万命令(callgrind、200〜1,600 関数)。フロントは 9 ms(JS だけ)から 25 ms ほどになった
  - 1 つの関数が N 個の callee を呼び、どれも所有に変わる例でも、関数 1 つあたり約 15〜18 万命令(N = 400、800)
    - 生存解析が、入れ子の演算子のたびに部分木の変数を数え直していて(O(n²))、それも式ごとに 1 回にした
  - 最初の版は全体を何度も lowering し直していて、鎖の例で 1.9 秒かかった(O(n²))
- 既存の例(numeric、shapes、ml、kernels、app)の wasm は、同じか小さくなった(numeric は 47,606 → 47,383 バイト。返す配列の引数を所有にしたので、関数の中の `.to_vec()` が消えた)

### 文字列と `==`

決めたこと:

- **`==` は中身で比べる**(配列、レコード、enum、タプル)。値は不変なので、中身が同じなら同じ値
  - JS では、型ごとに書いた比較関数(`$eq1` など)。Rust では `derive(PartialEq)`
    - 配列と typed array は要素ごと、レコードは宣言したフィールドごと、enum は TAG とそのバリアントのフィールドごと。ほかは `===`
    - 宣言したフィールドだけを比べるので、比較は対称になる。TS から来た値の余分なプロパティは無視する(境界でフィールドから値を作り直す wasm と同じ)
    - 再帰する型の比較関数は、自分自身を呼ぶ
  - 同じオブジェクトでも、近道で true にはしない。`[NaN] == [NaN]` は、Rust と同じく false
  - 比べられないのは、関数、JSX、effect と、数値の bound のない型変数(関数で具体化されうる)。どれも型エラー
- **文字列の長さと順序は JS に合わせる**: どちらも UTF-16 の単位。`"😀".len()` は 2
  - `"｡" < "😀"` は false(UTF-16 では U+FF61 が上、UTF-8 では逆)。wasm でも JS の順になる

`@wasm` の文字列(`examples/text.zsc`。JS と wasm の結果が一致する):

```zsc
type Person = { name: String, age: i32 }
@wasm fn greet(p: Person) -> String { "Hello, " + p.name + "!" }   // 左辺を取って push_str: 連結はその場で伸びる
@wasm fn longest(a: String, b: String) -> String { if a.len() >= b.len() { a } else { b } }
                                                   // fn zs_longest<'a>(a: &'a str, b: &'a str) -> &'a str
@wasm fn same(p: Person, q: Person) -> bool { p == q }           // (p == q): &ZsPerson どうし
```

- 文字列は Copy でない値として、配列と同じ所有権の推論に乗る(引数は `&str` で借り、消費するなら `String`、借用の結果は `&'a str`)
  - リテラルは `&'static str` として扱い、消費される所でだけ `String` にする
- `+` は左辺を所有して `push_str` する。左辺が最後の使用なら move するので、`s = s + x + ";"` はその場で伸びる
- JS と同じ意味にするための補助関数(zustc の std には `encode_utf16` と `memmem` がないので、自前で書く)
  - `zs_len16`(UTF-16 の長さ)、`zs_cmp16`(UTF-16 の順)
  - `zs_trim`(JS の空白と行終端。Rust の `trim` とは U+0085 と U+FEFF が違う)
  - `zs_contains`、`zs_split`(空の区切りは文字ごと)
  - `to_upper` は Rust の `to_uppercase`(どちらも Unicode の既定の大文字化)
- 分かっている違い: 空の区切りの `split` は、U+FFFF より上の文字を、JS では 2 つの単位に分け、wasm では 1 文字のままにする

テンプレート文字列(文字列の補間):

```zsc
@wasm pub fn card(p: Person, n: i64) -> String {
  `${p.name} (${p.age}): ${n * 2}`
}
// JS(--all-js):  return `${p.name} (${p.age}): ${BigInt.asIntN(64, n * 2n)}`;
// Rust:           { let mut __zs_t = String::new(); __zs_t.push_str(&p.name[..]); __zs_t.push_str(" ("); zs_push_int(&mut __zs_t, (p.age) as i64); .. }
```

- `${..}` に書けるのは、文字列と、`to_string` を持つもの(整数、浮動小数点数、bool、`Num` などの bound つきの型変数)。書き方は `to_string` と同じ
  - 配列、レコード、enum などは型エラー(何を書くかを選んでから `${..}` に入れる)
- JS ではテンプレートリテラルのまま出す。`@wasm` では `String` に順に `push_str` していき、穴は借りて渡す
  - **`format!` は使わない**: `core::fmt` が wasm に入ると、それだけで大きくなる。整数は自前の `zs_push_int` で書く(整数の `to_string` も同じ)
  - zustc で測った wasm のサイズ(1 つの関数のモジュール):

    | 本体 | `format!` / `to_string` | `push_str` / `zs_push_int` |
    |---|---:|---:|
    | `` `hi ${name}!` `` | 24,948 B | 19,505 B |
    | `` `hi ${name} ${n}` `` | 32,902 B | 23,775 B |
    | `"hi " + name + " " + n.to_string()` | 34,704 B | 27,316 B |
    | (比較: `"hi " + name + "!"`) | 21,047 B | 21,047 B |

  - 手書きの `wasm fn`(Rust そのまま)では `format!` も使えるが、同じ費用がかかる
  - `@wasm` の中の浮動小数点数の穴はエラー(JS と Rust で書き方が違う。`to_string` と同じ扱い)
- エスケープは文字列リテラルと同じ(`\n` `\t` `\\`)に加えて、`` \` `` と `\$`。`{` だけ、`$` だけは普通の文字

**ここから見えた現実的なライン**:

- f64 のループは、JS に `for` で出せば V8 の JIT が十分に速い。今の zustc の wasm 出力(振り分けの形、アドレス検査あり)とコピーの分を考えると、JS のほうが速い
- wasm が明確に勝つのは i64 / u64 を多用する計算(JS では BigInt になる)と、整数の wrap を多用する計算
- 小さい関数を何度も呼ぶ場合は、境界のコスト(スロットへの分解、`DataView` での読み戻し、オブジェクトの生成)が支配的になる。V8 は JS 同士ならインライン化できるので、JS のほうが速い
- したがって `@wasm` は「速くするための既定」ではなく、使いどころを選ぶ道具になる
  - 次の段階として、コンパイラがコピーの量や呼び出しの回数から判断材料を出すこと(候補 2)の価値は高い
  - zust の wasm バックエンドの改善(stackifier、wasm32 レイアウト)は、この境界線を wasm 寄りに動かす

### `@simd fn`: v128 で処理する配列カーネル

```zsc
@simd export fn dot(a: Array<f64>, b: Array<f64>) -> f64 {
  range(0, a.len()).fold(0.0, (s, i) => s + a[i] * b[i])      // f64x2: 2 要素ずつ
}
@simd export fn isum(xs: Array<i32>) -> i32 { xs.fold(0, (acc, x) => acc + x) }   // i32x4: 4 要素ずつ
@simd export fn norms(xs: Array<f64>, ys: Array<f64>) -> Array<f64> {
  range(0, xs.len()).map((i) => (xs[i] * xs[i] + ys[i] * ys[i]).sqrt())
}
```

- zustc の IR にはベクタ型がない。本体に手を入れない方針なので、**zsc が wasm のバイナリを直接書く**(`simd.rs`)
  - 例の 7 カーネルで 1.2 KB。zustc の出力は実行時の部分を含めて 30〜50 KB
  - JS に base64 で埋め込むので、別ファイルを読み込む必要がない
- **メモリはアロケータなし(呼び出しごとのアリーナ)**
  - glue が呼び出しごとに、引数の配列をオフセット 0 から 16 byte 境界で並べる
  - 足りなければ `memory.grow` で広げ、結果をコピーして取り出す
  - wasm 側には malloc も free もない
- 書けるカーネル
  - 形: スカラーの `let` を前に置いたうえで、`xs.map` / `xs.fold(init, (acc, x) => acc + ..)`、またはそれを `range(0, a.len())` の上で `a[i]` を読みながら書いたもの
  - 演算: `+ - *` と単項の `-`。f64 ではこれに加えて `/`、`sqrt`、`abs`
  - 型: 1 つのカーネルの中の配列とスカラーは、すべて f64 かすべて i32
  - 外れたものは、理由を示してエラーにする: i32 の割り算(wasm にベクタの整数除算がない)、和でない fold、型の混在など
- コード生成
  - ベクタのループのあとに、残りの要素を 1 つずつ処理するループを置く
  - スカラーは、ループの前に 1 回だけ `splat` してベクタにする
  - 配列が短いときは、ループの前に glue が RangeError を投げる(JS の `$at` と同じ扱い)
- 差分テスト `kernels_simd_nosimd_js_agree`: `@simd`、`--no-simd`(zustc を通したスカラーの wasm)、`--all-js` の 3 つの出力が一致する。f64 の和だけは 12 桁で比べる。1e6 要素の内積で、JS との差は 1.5e-15 だった

速さ(1e6 要素、Node 26.13。JS 側も `for` ループで出すようにした後の値。入力は普通の配列)

| カーネル | `@simd` | `--no-simd`(zustc) | JS |
|---|---|---|---|
| `dot` | 4.3 ms | 25.9 ms | 7.0 ms |
| `sum` | 1.2 ms | 17.4 ms | 0.9 ms |
| `axpy`(配列を返す) | 7.4 ms | 41.6 ms | 7.2 ms |
| `norms`(sqrt) | 12.1 ms | 47.5 ms | 16.0 ms |
| `isum`(i32) | 0.9 ms | 18.7 ms | 7.7 ms |
| `dot`、入力が `Float64Array` | 2.3 ms | 20.1 ms | 12.9 ms |

#### f32x4、relaxed SIMD、行列(ML 向け。examples/ml.zsc)

```zsc
// 全結合層: y = W x + b(W は m×n を行ごとに並べたもの)
@simd export fn dense(w: F32Array, x: F32Array, b: F32Array, m: i32, n: i32) -> F32Array {
  range(0, m).map((r) => b[r] + range(0, n).fold(0.0, (s, i) => s + w[r * n + i] * x[i]))
}
@simd(relaxed) export fn dense_fma(..) -> F32Array { /* 同じ本体 */ }   // 積和を relaxed_madd(FMA)に
@simd export fn relu(xs: F32Array) -> F32Array { xs.map((x) => x.max(0.0)) }
```

- **f32x4**: `F32Array` のカーネルは 4 レーンで処理する。f64x2 / i32x4 と同じ形で書ける
- **2 重のループ**: `range(0, m).map((r) => ..)` の本体の中に、内側の和を 1 つ置ける。内側では `w[r * n + i]` のように、行のオフセットを付けて読める
  - オフセットは、外側の `r` について 1 次式(`r * n + k`、i32 の引数とリテラル)に限る
  - 1 次式なので、glue は `r = 0` と `r = m - 1` の 2 点だけで、全部の読み出しが範囲内かを確かめられる。範囲外なら、ループの前に RangeError にする
  - i32 の引数は、f32 / f64 のカーネルの中でも、個数とオフセットとして使える
- **relaxed SIMD**(`@simd(relaxed)`): 結果がエンジンや CPU で変わりうる命令を、明示したときだけ使う
  - `a * b + c` と、内側の和の `s + a * b` は `f32x4.relaxed_madd` / `f64x2.relaxed_madd` になる(1 回丸めか 2 回丸めかはエンジンが選ぶ)
  - `max` / `min` は `relaxed_max` / `relaxed_min` になる(NaN と ±0 の扱いはエンジンが選ぶ)
  - relaxed でない `max` / `min` は、JS の `Math.max` と同じく、NaN を伝え、+0 を -0 より大きいとする。Rust に落とすときも、この意味の helper(`zs_max_f32` など)になる
- 差分テスト `ml_kernels_agree`: `@simd`、`--no-simd`、`--all-js` が一致する(和と relaxed の結果は 5 桁で比べる)。`max(-0, 0)` が `+0` になることや、範囲外の行数で RangeError になることも確かめている
- tsc の検査: `dense` などの `.d.ts` は `Float32Array` と branded な `i32` を取る。素の配列や素の number は型エラーになる

| カーネル(Node 26.13、入力は `Float32Array`) | `@simd` | `@simd(relaxed)` | `--no-simd`(zustc) | JS |
|---|---|---|---|---|
| `dense` 512×512(呼び出しごとに W をコピー) | 0.14 ms | 0.17 ms | 4.83 ms | 3.51 ms |
| `relu` 1e6 | 3.14 ms | 2.27 ms | 57.7 ms | 22.2 ms |
| `saxpy` 1e6(relaxed: FMA) | - | 2.30 ms | 39.7 ms | 15.7 ms |
| `sumsq` 1e6 | 0.67 ms | - | 22.4 ms | 6.23 ms |

- typed array を入力にすると、境界は `.set()` / `.slice()` の一括コピーだけになり、SIMD の差がそのまま出る: 全結合層は JS の 25 倍、ReLU は 7〜10 倍、saxpy は 7 倍
- relaxed は ReLU で効く(NaN と ±0 の処理を省ける)。FMA はこの CPU と V8 の組み合わせでは差が出なかった(全結合層は W のコピーが支配的)
- zustc を通したスカラーの wasm は、どれも JS より遅い。zust の IR にベクタ型を入れる件は [mizchi/zust#30](https://github.com/mizchi/zust/issues/30) にした

- 内積は 3 倍、i32 の和は 9.5 倍、sqrt を含む要素ごとの計算は 1.5 倍、それぞれ JS より速い
- 単純な和や `axpy` は、コピーの分で JS と同じくらいになる
- `--no-simd`(今の zustc の出力)は、どのカーネルでも JS より遅い。zust の wasm バックエンドの改善と、SIMD 用の IR の追加が効く場所はここ
- 配列を返すときの取り出し: `Array.from(typed)` は 1e6 要素で 45 ms、ループでのコピーは 14 ms、`typed.slice()` は 4.4 ms。glue はループでコピーするようにした。数値の配列を typed array のまま JS 層に返せば(`readonly T[]` と両立させる方法を決める必要がある)、map 型のカーネルもさらに速くなる
- 同時に JS の出力も改善した: `xs.fold` と `range(..).fold/map` を `reduce` や配列の生成ではなく、数え上げのループの helper にした。`sum` は 15.5 ms → 0.7 ms

## 測ったこと

試作のコンパイラの規模は約 3,900 行(parser 1,000、検査 1,350、JS 生成 1,000、wasm/グルー 280)。

| 項目 | 値 |
|---|---|
| zsc のフロント(parse、検査、JS と .d.ts の生成)、examples/app.zsc | 0.4 ms |
| zustc による wasm 層のコンパイル(7 関数) | 37 ms |
| app.js | 5.0 KB(gzip 1.8 KB) |
| app.wasm(既定、-O なし) | 36 KB(gzip 12.6 KB) |
| app.wasm(`--wasm-O`) | 54 KB(zustc の -O はインライン化でかえって大きくなる) |

### wasm のメモリモデルとコード生成量

参考: dandyvica/wasm_rust の要点は、`#[no_mangle] extern fn` でスカラーだけを渡し、`wasm-gc` で未使用のコードを落とすと 1.4 MB が 17 KB になること、そして wasm-bindgen の出力は肥大すること。

zust は到達しない関数をそもそも出さないので、`wasm-gc` に当たる処理は最初から入っている。残っているコストは次のとおり(examples と試作の出力で測った)。

1. **アドレス検査がコードの 27〜38%**
   - examples/collections 254 KB のうち 38%、enums 27%、structs 29%、app.wasm 30%
   - ポインタと usize が 64 bit(x86-64 のレイアウトのまま)なので、load/store ごとに「32 bit に収まるか、null ページでないか」を約 20 byte で検査している
   - wasm32 のレイアウト(ポインタ 4 byte)にすれば wrap が起きなくなり、境界検査は wasm 自身がするので、この検査はほぼ不要になる。null ページの検査も debug の時だけでよい
2. **`x + 1` の関数が 72 byte**(rustc なら 10 byte 前後)
   - ブロックごとの `loop` + `br_table` の振り分けと、値ごとの local が原因
   - stackifier / relooper と local の再利用で縮む(zust の tests/wasm/README でも次の手として挙がっている)
3. **実行時の基礎部分が約 11 KB**
   - `main`、引数、stdio、パニックの経路、アロケータ
   - `main` を持たないライブラリ形式(reactor)の出力を足せば、大半が消える
4. **境界のメモリ**
   - 試作では malloc/free を使っている
   - 呼び出しごとのアリーナ(呼び出しの間だけ使い、戻ったらまとめて捨てる)にすれば、グルーも wasm 側も短くなる

1〜3 は zust の `crates/wasm` とレイアウト(sema)の変更になるので、今回の方針(本体に食い込ませない)に従い、試作には入れていない。見積もり:

| 項目 | 目安 |
|---|---|
| reactor 出力 | 数日 |
| stackifier | 1〜2 週 |
| wasm32 レイアウト(usize とポインタを 4 byte に。レイアウト計算、ABI、prelude の前提に広く関わる) | 2〜4 週 |

合わせれば、wasm の島は今の半分以下になる見込み。

## 方向性の比較(試作から見えた現実的なライン)

| 方向 | 中身 | 試作で分かったこと | 判断 |
|---|---|---|---|
| **C. Rust 風の文法から JS を吐き、必要な所だけ wasm(今回の試作)** | JS 層は GC 前提で読める JS。wasm 層は Rust のまま | 1 回の作業で、型推論、JSX、effect、単相化、境界の自動生成まで動いた。**zust の型付き IR は不要だった**(JS 層は独自の検査器で、zust には wasm 層を Rust のテキストとして渡すだけ) | **本線にする** |
| A. Rust に変換できる高水準言語 | JS 層のコードを Rust ソースに落とし、zustc で wasm/native にする | 不変のレコードや enum は Rust に落とせる。ただしクロージャ、GC 前提の共有(`Rc` だらけになる)、effect(Rust にはジェネレータがない)を落とすのが難しい | **部分的に採り、`@wasm fn` として試作済み**: 数値と配列の部分集合を Rust に落とし、JS と wasm で同じ結果になることを差分テストで確かめた。ここでも zust の型付き IR は不要だった(zustscript の型付き構文木と、検査器の表で足りた) |
| B. 全部 wasm に変換する前提で効率化 | UI まで wasm に置き、メモリモデルを wasm 向けにする | DOM には JS 経由でしか触れないので、境界の量は C より増える。サイズは 1〜4 の改善をしても JS 1.8 KB(gzip)に対して桁が違う | **単独の方向にはしない**。1〜4 の改善は C の wasm の島にそのまま効くので、別の作業として進める価値はある |

結論:

- C を本線にする
- A は「JS 層の純粋な数値の部分集合を wasm にも落とせる」という形で C に合流させる
- B のメモリモデルの改善は zust 本体の wasm バックエンドの課題として切り出す

## 段階的な目安(見積もりの更新)

| 段階 | 内容 | 目安 | 備考 |
|---|---|---|---|
| 済 | 試作(この crate): JS 層、effect、JSX、境界、branded な `.d.ts`、`@wasm fn` | - | 型付き IR なしで成立 |
| 1 | 言語として固める: モジュールと import、無名のレコード型、`Option`/`Result` の標準化、文字列の補間、エラーの改善、TS からの型名の衝突回避 | 2〜3 週 | |
| 2 | 道具: 増分ビルド(アイテム単位のハッシュは zust の lexer の考え方を流用)、LSP(診断、hover、定義へ移動)、source map | 3〜5 週 | zust の `ide` / `analyzer` の作りを参考に別 crate で |
| 3 | effect 多相(関数型の effect 行)、async の effect(Promise で継続を再開) | 2〜4 週 | 生成はジェネレータのまま |
| 4 | 方向 A の合流の続き: どの関数を wasm に置くかの判断材料(コピーの量と呼び出しの回数の見積もり)、`range(..).fold` などの JS 側での `for` への最適化、文字列やタプルへの部分集合の拡張、データを wasm 側に置いたまま繰り返し処理する仕組み(ハンドル) | 2〜4 週 | 数値、レコード、enum、ループまでは試作済み |
| 5 | wasm バックエンドの改善(reactor、stackifier、wasm32 レイアウト) | 1〜2 か月 | zust 本体の作業 |
| 6 | TS の型の取り込み(限定した部分集合、tsc API による抽出) | 範囲次第 | プレーンなレコード型は `import type` で読める。関数は手書きの extern |

- 小さな実アプリ(React 上)が書けるまで: 段階 1〜2 で 1.5〜2 か月
- wasm の島が実用サイズになるまで: 段階 5 が必要

## 未決事項

- `@simd` で書けるカーネルの拡張
  - 比較とマスク(`if` を `bitselect` に)、複数の出力
  - 行列と行列の積(3 重のループ、タイル分割)
  - int8 の量子化(`i32x4.relaxed_dot_i8x16_i7x16_add_s`)。そのためには `I8Array` / `i8` が要る
- 呼び出しごとのコピーをなくす: 重みのように何度も使う配列を、wasm 側のメモリに置いたままにするハンドル(`WasmF32Array` など)
- typed array を不変として扱うことの徹底: JS の `Float32Array` は書き換えられるので、TS 側から渡された配列が後で変わりうる。今は zustscript 側からは変更できない、というだけ
- 配列の更新が多い箇所の計算量: `readonly T[]` に決めたので、`push` は O(n) のコピーになる。ループのように積み上げる用途には、`fold` で一度に作る、あるいは wasm 層に寄せる方針で足りるかを確かめる
- brand のコンストラクタ名(`i32` など)が、利用者の関数名と衝突しうる。今は検査していない
- wasm 層から JS 層の関数(コールバックでない普通の関数)を呼ぶ方向
- wasm 側で panic したときに所有コールバックが漏れること(今は abort して例外になり、表に残る)
- 試作の割り切り
  - リテラルパターンの i64 比較は `==` で済ませている
  - 引数の位置に置いた `match` は即時関数になる(評価順は保たれる)
