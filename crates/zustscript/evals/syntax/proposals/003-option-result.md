# 003: 標準の Option / Result と、`?` 演算子、任意のフィールド

状態: 草案 (r1)

## 動機

`corpus/options.zsc` より。`examples/shapes.zsc`、`basics.zsc`、`regress.zsc` も同じ形。

1. **`Option` / `Result` をファイルごとに宣言している**
   - どのファイルも `enum Option<T> { None, Some(T) }` を書く
   - 001 の r4 の既定値(`let mut best = None` が `Option` になる)は、標準の `Option` があることを前提にしている
2. **小さな関数をファイルごとに書き直す**
   - `map_opt`、`unwrap_or` を毎回書く。ユーザーの enum にはメソッドを書けないので、関数で書くしかない
   - 呼ぶ側は `unwrap_or(map_opt(find(todos, id), (t) => t.title), "(none)")` と、内側から読むことになる
3. **エラーを渡すだけの match が、段ごとに 1 段深くなる**
   - `validate` と `rename` は、`Err(e) => Err(e)` を書くための match が半分を占める
   - 本当にしたい処理(`Ok(title) => …`)が、いちばん深い所に来る
4. **無いかもしれない prop も、毎回渡す**
   - `TodoRowProps` の `subtitle: Option<String>` は、無いときも `subtitle={None}` と書く
   - TS 側からは `{ TAG: "None" }` を作って渡すことになる(`.d.ts` は `readonly subtitle: Option<string>`)
   - 002 は、`title?: T` をこの提案に回した

この文書では、`(…)` / `{ … }` は「中身を省略した」という意味で使う。コードとしての `...` は、001 / 002 と同じく spread とパターンの残りだけ。

## 提案

### A. prelude の Option / Result

- 次の 2 つは、宣言しなくても使える(prelude)

```zsc
enum Option<T> { None, Some(T) }
enum Result<T, E> { Ok(T), Err(E) }
```

- ふつうの enum と同じに扱う
  - match、網羅性、修飾(`Option::None`、`Result::Ok(x)`)も同じ
  - 型引数は推論する。`Option::<i32>::None` とは書かない(001 の未決事項への答え)
- **名前の衝突**
  - 型の名前 `Option` / `Result` は予約する。同じ名前の `enum` / `type` を宣言するとエラー
  - ただし、prelude と同じ形の `enum Option<T> { None, Some(T) }` だけは警告にする(移行のため)
    - 直し方: 宣言を消す。LSP のクイックフィックスにする
  - バリアント名 `None` / `Some` / `Ok` / `Err` は、ユーザーの enum も持てる(001 のまま。`Parsed::None`)
- **修飾しないバリアント名の既定値**(001 r4 の決定 5。この規則は 003 が持つ)
  - 001 の「期待する型による解決」で決まらないとき、`None` / `Some` / `Ok` / `Err` は prelude の型になる
  - 整数リテラルが、決まらなければ i32 になるのと同じ
  - 引数のあるもの(`Some(1)`、`Ok(s)`)
    - 期待する型がなければ、その場で prelude の enum として推論する
    - 期待する型が、まだ決まっていない候補つきの型変数のときも、期待する型がないものとして扱う
  - 引数のない `None` は、001 のとおり関数の終わりまで遅らせ、それでも決まらなければ `Option` にする
    - ただし、決まっていない値を match にかける、フィールドを読む、メソッドを呼ぶときは、候補に prelude の enum があれば、その時点で既定値に決める(001 の r4 の指摘の扱い。小数リテラルが f64 になるのと同じ)
  - 期待する型が、同じ名前のバリアントを持つユーザーの enum なら、そちらが勝つ(001 の `describe` の `None`)
  - prelude は固定なので、enum を足しても離れた場所の意味は変わらない
  - prelude に決まったことが原因の型エラーには、「期待する型がないので、ここで `Result::Ok` に決まった」という注を付ける
- `Option` / `Result` は、effect の enum にはならない(バリアントに `-> R` がないため)。effect の値として渡すのは今までどおり

### B. `?` 演算子

```zsc
fn validate(d: Draft, id: i32) -> Result<Todo, FormError> {
  let t = non_empty("title", d.title)?;    // Err なら、その Err を validate の結果として返す
  let title = at_most("title", t, 80)?;
  Ok({ id, title, done: false })
}
```

- **書き方**: 式の後ろに `?`。後置の演算子で、呼び出し、`.f`、`[i]` と同じ強さ
  - `a.b()?.len()` のような `?.` の並びは、r1 ではエラー(下の「予約する字面」)
- **意味**
  - `e?` の e が `Result<T, E>` のとき
    - `Ok(x)` なら、式の値は `x`
    - `Err(err)` なら、その `Err(err)` を、いちばん内側の関数(またはクロージャ)の結果として返す
  - `e?` の e が `Option<T>` のとき
    - `Some(x)` なら `x`。`None` なら `None` を返す
  - ほかの型(ユーザーの enum を含む)には使えない。Rust の `Try` に当たる仕組みは入れない
- **型の規則**
  - 囲む関数の戻り値の型を R とする(クロージャの中では、そのクロージャの戻り値の型)
  - R が `Result<U, E>` なら、e は `Result<T, E>` でなければならない
    - **E は完全に一致する**。`From` のような変換はしない
  - R が `Option<U>` なら、e は `Option<T>` でなければならない
  - **e の期待する型**: R が `Result<U, E>` なら `Result<_, E>`、`Option<U>` なら `Option<_>`
    - 001 の「期待する型が伝わる場所」に、`?` の左の式を足す
    - そのため `find(todos, id).ok_or(NotFound { id })?` の `NotFound` も、戻り値の型から決まる(D の「期待する型」)
  - e の型は、`?` の時点で `Option` か `Result` に決まっていなければならない。ふつうの型変数ならエラー(注釈を求める)
- **誤りと直し方**(すべてコンパイル時)
  - E が違う
    - ``error: `?` passes on a `FormError`, but `load` returns `Result<i32, String>` ``
    - ``(no conversion is done: convert it first, e.g. `validate(d, 1).map_err(message)?`)``
  - `Result` の関数で `Option` に `?`
    - ``error: `?` on an `Option` in a function that returns `Result`: turn it into a `Result` with `.ok_or(e)?` ``
  - `Option` の関数で `Result` に `?`
    - ``error: `?` on a `Result` in a function that returns `Option`: use `match` to drop the error``(`.ok()` は未決事項)
  - 戻り値が `Option` でも `Result` でもない
    - ``error: `?` can be used only in a function that returns `Option` or `Result`; `TodoRow` returns `Jsx` ``
  - LSP: E が違うときは `.map_err(…)` を足すクイックフィックス、`Option` のときは `.ok_or(…)` を足すクイックフィックス
- **クロージャ**
  - **クロージャの中の `?` は、そのクロージャから返る**(Rust と同じ。JS の `return` とも同じ)
  - 外の関数からは返らない。外の関数から返す書き方はない
  - クロージャの戻り値の型が分かっていないときは、`?` がそれを `Result<_, E>`(または `Option<_>`)に決める
  - 誤りやすい例と、そのエラー

```zsc
fn titles(ds: Array<Draft>) -> Result<Array<String>, FormError> {
  Ok(ds |> .map((d) => non_empty("title", d.title)?))
  //                    ^ error: this `?` returns from the closure, not from `titles`:
  //                      the closure must return a `Result` (its body is `String` here)
}
```

  - `Array<Result<T, E>>` を `Result<Array<T>, E>` にまとめる関数は、r1 では入れない(未決事項)。`for` と `let mut` で書く
- **effect(ジェネレータ)**
  - effect を持つ関数の中でも使える。JS では `function*` の中の `return` になり、`yield*` で呼んだ側に `Err` が値として返る
  - `perform` の結果にも付けられる。`perform` の後ろは操作の呼び出しなので、`(perform Ask(q))?` と括弧で囲む
    - 括弧なしの `perform Ask(q)?` は、直し方つきのエラー(操作の値そのものに `?` を付けたように読めるため)
  - **`handle` の本体と、handler の arm の中では、r1 では `?` を書けない**
    - JS では、どちらも `$handle` に渡す別のジェネレータなので、`return` が囲む関数から返らない
    - ``error: `?` cannot be used inside `handle` yet: bind the result with `let r = handle …;` and use `r?` after it``
    - どちらの関数から返るべきかは未決事項
- **`|>`**
  - `x |> f(a)?` は `(x |> f(a))?`、つまり `f(x, a)?`
    - `?` は、脱糖した後の呼び出しの結果に付く。`f(a)` の結果に付いて、それを呼ぶ、とは読まない
  - `x |> .m(a)?` も同じく `x.m(a)?`
  - 続けて書ける: `s |> trimmed()? |> shorter_than(80)?` は `shorter_than(trimmed(s)?, 80)?`
  - `x? |> f()` は `f(x?)`
- **`@wasm`**: Rust の `?` にそのまま下ろす(F)。E が完全に一致するので、Rust の `From` は恒等の変換になり、意味は JS と同じ
- **評価順**: `f(a(), g()?)` で `g()` が `Err` なら、`a()` は評価済み、`f` は呼ばれない(左から右。JS と Rust と同じ)
- **予約する字面**(r1 ではエラーにして、意味を後で決める)
  - `e?.f`、`e?.m()`
    - Rust の人は「`e?` してから `.f`」、TS の人は「optional chaining」と読む。意味が違うのに両方通ると、静かに読み違える
    - ``error: `?.` is reserved: write `(e?).f` to return early, or `e.map((x) => x.f)` for a value that may be absent``
  - `e ?? d`: ``error: `??` is reserved: write `e.unwrap_or(d)` ``
  - `e??`(`Option<Option<T>>` の 2 段の `?`): `(e?)?` と書く

### C. 任意のフィールド `title?: T`

```zsc
type Todo = { id: i32, title: String, note?: String, done: bool }
```

- **型の規則**
  - `note?: String` のフィールドを読むと、型は `Option<String>`
    - `t.note`、分割代入の `{ note }`、match のパターン、spread の上書き、どれも `Option<String>` として扱う
  - 書くときの値の型も `Option<String>`
    - **暗黙に `Some` で包まない**: `{ note: "x" }` はエラー。`{ note: Some("x") }` と書く
    - 数値と同じく、暗黙の変換はしない方針に合わせる。直し方(`Some(…)` で包む)つきのエラーと、クイックフィックス
  - `T` に `Option<…>` と `()` は書けない(`note?: Option<String>` はエラー)
    - JS では無いことを `undefined` で表すので、`None` と `Some(None)`、`Some(())` の区別がつかないため
    - 総称の `{ value?: T }` で、T を `()` や `Option<…>` にしたときも、その場でエラー
- **省略**
  - レコードのリテラルで、任意のフィールドは省略できる。省略したら `None`
  - JSX の属性でも省略できる(`jsx()` の「足りない prop」の検査から外す)
  - **省略できるのは、期待する型が分かっている所だけ**(001 の「期待する型が伝わる場所」)
    - 期待する型がない `let r = { id: 1, title: "a", done: false };` の型は `{ id, title, done }` のまま
    - それを `Todo` を受ける関数に渡すとエラー(002 の、幅の部分型なしの規則)
    - ``error: missing field `note` of `Todo` (an optional field can be left out only where the type is known: annotate `let r: Todo = …`)``
- **型の等しさ**
  - `{ note?: String }` と `{ note: Option<String> }` は、**別の型**
  - 中で読み書きする型は同じ `Option<String>` だが、JS での持ち方(下の「出力」)が違うため
  - 単一化の差分の表示で、`note?:` と `note:` の違いを「任意かどうか」として示す
- **書ける場所**: レコードの型(`type` の別名、無名のレコード型、`@nominal type`)。enum のバリアントの名前付きフィールドと、関数の引数には、r1 では書けない(未決事項)
- **TS との境界**(JS の `undefined` と `None`)
  - zustscript の中の値は、任意のフィールドに **`T` か `undefined`** を持つ(`{ TAG: "Some" }` の形にはしない)
  - TS から来た値
    - プロパティが無い、`undefined`、`null` は、どれも `None`(読むときに `== null` で比べる)
      - `null` は `.d.ts` の型では渡せないが、JS から来ても `None` として扱う
    - それ以外は `Some(値)`
    - 境界でオブジェクトを作り直さない(配列の中のレコードも、そのまま渡せる)
  - TS へ渡す値
    - レコードのリテラルは、`None` のフィールドも `note: undefined` としてキーを書く(省略したときも)
      - spread の上書き `{ ...t, note: None }` で、元の値を確実に消すため
      - オブジェクトの形がそろうので、JS エンジンにも扱いやすい
    - JSX の props は、省略したら書かない(React の慣習どおり)
  - `.d.ts` は `readonly note?: string | undefined`
    - `?` があるので、TS 側は省略できる
    - `| undefined` を付けるのは、出力が `note: undefined` を書くため。`exactOptionalPropertyTypes` を有効にした TS でも、読んだ値の型が合う

### D. 基本のメソッド

組み込み(`Array` の `map` と同じく、検査器が知っている)にする。言語には、ユーザーの型にメソッドを定義する方法がないため。名前は Rust と同じ。

| 受け手 | メソッド | 型 |
|---|---|---|
| `Option<T>` | `map(f)` | `f: (x: T) => U` → `Option<U>` |
| | `and_then(f)` | `f: (x: T) => Option<U>` → `Option<U>` |
| | `unwrap_or(d)` | `d: T` → `T` |
| | `ok_or(e)` | `e: E` → `Result<T, E>` |
| | `is_some()` / `is_none()` | → `bool` |
| `Result<T, E>` | `map(f)` | `f: (x: T) => U` → `Result<U, E>` |
| | `and_then(f)` | `f: (x: T) => Result<U, E>` → `Result<U, E>`(E は同じ) |
| | `map_err(f)` | `f: (e: E) => F` → `Result<T, F>` |
| | `unwrap_or(d)` | `d: T` → `T` |
| | `is_ok()` / `is_err()` | → `bool` |

- `map_err` は、`?` の E をそろえる方法として足した(B)。これがないと、変換に毎回 match が要る
- `unwrap_or(d)` の `d` は、いつも評価される(Rust と同じ)。`unwrap_or_else` は未決事項
- `unwrap()` / `expect()`(無いときに止める)は、r1 では入れない(未決事項)
- **型の検査**
  - 受け手の型は、呼ぶ時点で `Option` / `Result` に決まっていなければならない(001 の「決まっていない値は、使う前に決める」)
    - `Some(1).map(…)` は通る(`Some(1)` はその場で prelude の型になる)
    - `let x = None; x.is_none()` は、その時点で `x` が `Option<_>` に決まって通る(A の既定値)
    - 候補に prelude の enum がない値(ユーザーの enum のバリアントだけ)は、001 のとおり修飾を求めるエラー
  - **期待する型**: 呼び出しの期待する型を、先に戻り値の型と単一化してから、引数を検査する
    - `find(todos, id).ok_or(NotFound { id })?` では、`?` から来た `Result<_, FormError>` が `ok_or` の E を決め、`NotFound` が `FormError` に決まる
  - クロージャの引数の型は、受け手から決まる(`Array` の `map` と同じ)
  - effect を持つクロージャは渡せない(今の規則のまま)
- `|>` でも書ける: `find(todos, id) |> .map((t) => t.title) |> .unwrap_or("(none)")`
- `is_some()` で `if` に分けても、中で値は取り出せない(絞り込みはない)。取り出すには match を使う(`if let` は未決事項)

## 変更前 / 変更後(corpus/options.zsc)

変更前は `corpus/options.zsc`。変更後は、001 と 002 も入った形で書く。

```zsc
// 変更後: Option / Result の宣言、map_opt、unwrap_or はなくなる
enum FormError { Empty { field: String }, TooLong { field: String, max: i32 }, NotFound { id: i32 } }

type Todo = { id: i32, title: String, note?: String, done: bool }
type Draft = { title: String, note: String }

fn find(todos: Array<Todo>, id: i32) -> Option<Todo> {
  let hits = todos.filter((t) => t.id == id);
  if hits.len() == 0 { None } else { Some(hits[0]) }   // 戻り値の型 Option<Todo> で決まる
}

fn non_empty(field: String, s: String) -> Result<String, FormError> {
  let t = s.trim();
  if t.len() == 0 { Err(Empty { field }) } else { Ok(t) }   // Empty は Err の引数の期待する型 FormError で決まる
}

fn at_most(field: String, s: String, max: i32) -> Result<String, FormError> {
  if s.len() > max { Err(TooLong { field, max }) } else { Ok(s) }
}

fn validate(d: Draft, id: i32) -> Result<Todo, FormError> {
  let t = non_empty("title", d.title)?;
  let title = at_most("title", t, 80)?;
  let n = d.note.trim();
  let note = if n.len() == 0 { None } else { Some(n) };
  Ok({ id, title, note, done: false })        // Ok の引数の期待する型 Todo。note は Option<String>
}

fn rename(todos: Array<Todo>, id: i32, d: Draft) -> Result<Array<Todo>, FormError> {
  let old = find(todos, id).ok_or(NotFound { id })?;   // NotFound は ? の期待する型から FormError に決まる
  let t = validate(d, id)?;
  Ok(todos |> .map((x) => if x.id == id { { ...t, done: old.done } } else { x }))
}

pub fn title_of(todos: Array<Todo>, id: i32) -> String {
  find(todos, id) |> .map((t) => t.title) |> .unwrap_or("(none)")
}

pub fn message(e: FormError) -> String {
  match e {
    Empty { field } => field + " is empty",
    TooLong { field, max } => field + " is longer than " + max.to_string(),
    NotFound { id } => "no todo " + id.to_string(),
  }
}

pub fn TodoRow({ todo, subtitle }: { todo: Todo, subtitle?: String }) -> Jsx {
  <li>
    {todo.title}
    {match subtitle { Some(s) => <small>{s}</small>, None => <small /> }}
  </li>
}

pub fn TodoList({ todos }: { todos: Array<Todo> }) -> Jsx {
  <ul>{todos |> .map((t) => <TodoRow key={t.id} todo={t} subtitle={t.note} />)}</ul>
}

pub fn Placeholder() -> Jsx { <ul><TodoRow todo={{ id: 0, title: "nothing yet", done: false }} /></ul> }
//                                      ^ subtitle と note は省略。どちらも期待する型(props の型、Todo)が分かっている所
```

- `validate` と `rename` の match の入れ子がなくなり、本当にしたい処理が上から下へ並ぶ
- `title_of` は、左から右へ読める
- `TodoList` は `t.note`(`Option<String>`)を、任意の prop `subtitle` にそのまま渡す
- `Placeholder` は、任意のフィールドと任意の prop を省略する例

誤りの例。

```zsc
fn load(d: Draft) -> Result<Todo, String> {
  validate(d, 1)?
  // error: `?` passes on a `FormError`, but `load` returns `Result<Todo, String>`
  //        (no conversion is done: convert it first, e.g. `validate(d, 1).map_err(message)?`)
}

fn first_note(todos: Array<Todo>) -> Result<String, FormError> {
  Ok(todos[0].note?)
  // error: `?` on an `Option` in a function that returns `Result`: turn it into a `Result` with `.ok_or(e)?`
}

fn shout(t: Todo) -> String {
  t.note?.to_upper()
  // error: `?.` is reserved: write `(e?).f` to return early, or `e.map((x) => x.f)` for a value that may be absent
  // (しかも shout は Option を返さないので、`(t.note?)` も書けない)
}

fn draft_of(t: Todo) -> Todo {
  { ...t, note: "x" }
  // error: expected `Option<String>`, found `String` (`note` is optional: write `Some("x")`)
}
```

effect、`@wasm` と一緒に使う例。

```zsc
enum Log { Info(String) -> () }

fn save(d: Draft, id: i32) -> Result<Todo, FormError> {
  let t = validate(d, id)?;            // effect を持つ関数の中でも同じ
  perform Info("saved " + t.title);
  Ok(t)
}

@wasm fn safe_div(a: i32, b: i32) -> Option<i32> { if b == 0 { None } else { Some(a / b) } }
@wasm pub fn ratio(a: i32, b: i32, c: i32) -> Option<i32> {
  let x = safe_div(a, b)?;
  safe_div(x, c)
}
```

## 出力

### JS: Option / Result の値

- Option / Result の値の表現は、**データを持つ enum の表現の決定(`@tag`、docs/plan.md の「保留」)に従う**
  - この提案は、表現を決めない。今の表現なら、ユーザーが宣言した `Option` と同じ出力になる
    - `Some(x)` は `{ TAG: "Some", _0: x }`、`None` は `{ TAG: "None" }`
  - `@tag` の決定への条件は 1 つ: `None` と `Some(None)` を区別できること
    - `Option<T>` をいつでも `T | undefined` にする表現は、`Option<Option<T>>` と、総称の関数(JS では型を消す)で区別できなくなる
    - そのため、`undefined` を使うのは C の任意のフィールドだけにする(型で T が `Option` でないと分かる所)
  - 表現を決める処理は、今と同じく js.rs の `variant_value` と、wasm.rs の `VTy::Enum` にまとめる。`?` とメソッドの出力も、そこから作る
- **`?`**(今の表現で)

```js
// let t = non_empty("title", d.title)?;
const $t1 = non_empty("title", d.title);
if ($t1.TAG === "Err") return $t1;     // Err の値を作り直さずに返す(E が同じなので、同じ形)
const t = $t1._0;

// effect を持つ関数(function*)の中: 呼び出しが yield* になるだけ
const $t2 = yield* validate(d, id);
if ($t2.TAG === "Err") return $t2;

// 式の途中の ?: f(a(), g()?)。左の a() を先に一時変数に置き、評価順を保つ
const $t3 = a();
const $t4 = g();
if ($t4.TAG === "Err") return $t4;
return f($t3, $t4._0);
```

- `?` を含む式は、即時関数(`(() => { … })()`)にしない。即時関数の中の `return` は、囲む関数から返らないため
  - `if` / `match` / ブロックを、`let v = if …` と同じ文の形で書き出す
- クロージャの中の `?` は、アロー関数の `return` になる(クロージャから返る)
- **メソッド**: 小さな実行時ヘルパにする(`$handle` と同じ置き方。使うものだけ書き出す)

```js
// find(todos, id) |> .map((t) => t.title) |> .unwrap_or("(none)")
$unwrap_or($opt_map(find(todos, id), (t) => t.title), "(none)")

function $opt_map(o, f) { return o.TAG === "Some" ? { TAG: "Some", _0: f(o._0) } : o; }
function $unwrap_or(o, d) { return o.TAG === "Some" || o.TAG === "Ok" ? o._0 : d; }
```

### JS: 任意のフィールド

```js
// Ok({ id, title, note, done: false })      note は Option<String> の変数
return { TAG: "Ok", _0: { id, title, note: note.TAG === "Some" ? note._0 : undefined, done: false } };
// { id: 0, title: "nothing yet", done: false }   省略したとき
({ id: 0, title: "nothing yet", note: undefined, done: false })
// match t.note { Some(s) => …, None => … }   包まずに比べる
const $t1 = t.note;
if ($t1 != null) { const s = $t1; … } else { … }
// t.note を値として使うとき(関数に渡す、変数に入れる)だけ、包む
(t.note != null ? { TAG: "Some", _0: t.note } : { TAG: "None" })
// <TodoRow todo={t} />    省略した prop は書かない
_jsx(TodoRow, { todo: t })
```

- リテラルの `Some(e)` / `None` を任意のフィールドに書くときは、包まずに `e` / `undefined` を書く
- match、`?`、メソッドの受け手に直接 `t.note` を書いたときも、包まない
- 分割代入(`{ todo, subtitle }`)で変数に入れるときは、そこで一度だけ包む。その変数を match にだけ使うなら、包まずに済ませてよい(最適化。意味は変わらない)

### `.d.ts`

```ts
export type Option<T> = { readonly TAG: "None" } | { readonly TAG: "Some"; readonly _0: T };
export type Result<T, E> = { readonly TAG: "Ok"; readonly _0: T } | { readonly TAG: "Err"; readonly _0: E };

export type Todo = { readonly id: i32; readonly title: string; readonly note?: string | undefined; readonly done: boolean };
export declare function TodoRow(props: { readonly todo: Todo; readonly subtitle?: string | undefined }): $ReactElement;
export declare function title_of(todos: readonly Todo[], id: i32): string;
```

- `Option` / `Result` の型は、使うモジュールの `.d.ts` ごとに書き出す
  - 数値の brand と同じく、構造的な型なので、モジュールが違っても互換になる
  - 中身は、今ユーザーが宣言したときと同じ。移行しても TS 側は変わらない
  - 形は `@tag` の決定で変わりうる(001 の namespace `Option.Some` も同じ)
- TS から `Option` の値を作るコンストラクタは、001 と同じく `@tag` の決定で扱う

### Rust / wasm(`@wasm`)

- `Option<T>` / `Result<T, E>` は、Rust の標準の `Option` / `Result` に下ろす(`ZsOption` は作らない)
  - T と E が Copy なら Copy になるので、今の「レコードと enum は Copy で値渡し」と同じに使える
  - パターンとバリアントは `Some(x)` / `None` / `Ok(x)` / `Err(e)`。ユーザーの enum の `None` は、今までどおり `ZsParsed::None` と修飾して出すので衝突しない
- `?` は Rust の `?`。メソッドは Rust の同じ名前のメソッド(`map`、`and_then`、`unwrap_or`、`ok_or`、`map_err`、`is_some` …)
  - `map` / `and_then` / `map_err` のクロージャは、Rust のクロージャになる(今は配列のメソッドの引数だけなので、ここに足す)
- 境界
  - スロットへの分解は、今の `VTy::Enum` のまま。タグの番号は宣言の順(`None` = 0、`Some` = 1、`Ok` = 0、`Err` = 1)で、今の `ZsOption` と同じ
  - 組み立て直すときのパスだけが変わる(`ZsOption::Some(..)` → `Some(..)`)
  - 任意のフィールドは、JS 側で `== null` ならタグ 0、そうでなければタグ 1 と値のスロット。戻すときは `undefined` か値を書く
- `@wasm` の部分集合には文字列がないので、`Result<T, String>` は `@wasm` では使えない。エラーは数値やデータなしの enum で表す

```rust
fn zs_safe_div(a: i32, b: i32) -> Option<i32> { if b == 0 { None } else { Some(a.wrapping_div(b)) } }
fn zs_ratio(a: i32, b: i32, c: i32) -> Option<i32> {
    let x = zs_safe_div(a, b)?;
    zs_safe_div(x, c)
}
```

## 実装の注意

- **prelude**: check.rs の enum の表に、ユーザーの宣言より先に `Option` / `Result` を入れる。`prelude` の印を持たせる
  - js.rs と dts は、使われたものだけ `.d.ts` に書き出す。lower.rs は定義を書かず、`rs_type` を `Option` / `Result` にする
  - 既定値の規則は、001 の finish_fn の第 1 段(候補に prelude の enum があればそれに決める)のまま
- **構文**
  - lexer の `PUNCTS` に `?.`、`??`、`?` を足す(前から照合するので、この順)
  - postfix() で、後置の `?` を `EK::Try(e)` として読む。`?.` と `??` は予約のエラー
  - pipe() は、右辺を postfix() で読んだ後、末尾の `?` を脱糖した呼び出しの外側に付け直す
    - 今は右辺を `self.postfix()` で読むので、そのままだと `Call(Try(f(a)), [x])` になってしまう
  - 型の位置のレコードで、`ident ?:` を任意のフィールドとして読む
- **型検査**
  - 関数とクロージャの戻り値の型を、スタックで持つ(`EK::Closure` で積む)。`EK::Try` はその先頭を見る
  - `EK::Try` の左の式には、戻り値の型から作った `Result<_, E>` / `Option<_>` を期待する型として渡す
  - `handle` の本体と arm の中にいるかを持ち、そこでの `EK::Try` はエラー
  - method_sig に `Option` / `Result` の行を足す。期待する型を戻り値の型と先に単一化してから、引数を検査する
  - 002 の `Ty::Rec` のフィールドに「任意」の印を持たせる。等しさに含める。読むときの型は `Option<T>`
  - jsx() の「足りない prop」の検査と、レコードのリテラルの「足りないフィールド」の検査から、任意のフィールドを外す
- **JS の出力**
  - `?` を含む式は、即時関数にしない(`loop_exit` と同じく、含むかどうかを先に調べる)。文の形で書き、左の兄弟を一時変数に置く
  - 任意のフィールドの読み出しは、使われ方で包むかを決める(match、`?`、メソッドの受け手なら包まない)
- **effect の推論**: EffWalk は `EK::Try` の中を見るだけ(`?` は effect を足さない)
- 解決はすべて関数の中で閉じる。アイテム単位の増分検査は変わらない

## 代替案

1. **TS 風の nullable(`T | null` / `T | undefined`)を言語の型にする**
   - TS の人には近いが、`Option<Option<T>>` を区別できない。総称の関数で型を消すと、`T` 自体が null のときと区別できない
   - 失敗の理由を持てない(`Result` の代わりにならない)
   - `@wasm` でも、Rust の `Option` に素直に下ろせない
   - 選ばない。`undefined` は、型で区別がつく任意のフィールドの持ち方にだけ使う
2. **TS の `?.` と `??` を主な道具にする**
   - nullable の型が前提になる(1 と同じ問題)
   - 後置の `?` と字面が重なる。そのため r1 では両方を予約し、意味は未決事項にした
3. **`?` を例外で書き出す**(`$try(e)` が投げ、関数の入口の `try` で受ける)
   - 即時関数の中でも、評価順を気にせずに書ける
   - ただし失敗のたびに例外になり遅い。出力も読みにくく、ジェネレータの中での振る舞いも追いにくい
   - 選ばない。`return` で書き出し、即時関数を避ける
4. **`From` のような、エラーの型の自動の変換**
   - trait がない。暗黙の変換をしない方針とも合わない
   - 選ばない。`map_err` を明示して書く
5. **`title?: T` を、持ち方まで `title: Option<T>` と同じにする(完全な略記)**
   - `.d.ts` が `readonly title?: Option<string>` になり、TS からは `{ TAG: "None" }` を作って渡すことになる。省略はできても、値を渡すのが重い
   - 選ばない
6. **任意のフィールドを中では `{ TAG }` で持ち、境界で `undefined` と変換する**
   - 配列の中のレコードまで、境界で作り直すことになる。`readonly T[]` をコピーなしで渡す方針と合わない
   - 選ばない。中でも `T | undefined` で持つ
7. **任意のフィールドに、`T` の値をそのまま書ける(暗黙の `Some`)**
   - `<TodoRow subtitle="x" />` と書けて TS に近い
   - 期待する型で値の意味が変わる、暗黙の変換になる
   - r1 では選ばない。JSX の属性に限って許すかは未決事項
8. **メソッドを、prelude のソースに関数として書く**(`fn map<T, U>(o: Option<T>, …)`)
   - ユーザーの型にメソッドを書く方法がないので、`o.map(…)` と書けない。名前も利用者の関数(`regress.zsc` の `unwrap_or`)とぶつかる
   - 選ばない。組み込みにする
9. **`try { … }` ブロックや do 記法**: `?` の範囲を関数より狭くできるが、覚えることが増える。選ばない

## 未決事項

- `?.` を何に使うか
  - (a) Rust の意味(`(e?).f`)にする
  - (b) TS の optional chaining(`e.map((x) => x.f)` の略記)にする
  - (c) 予約のまま
- `??` を `unwrap_or` の略記として入れるか(右辺を遅らせて評価するなら `unwrap_or_else` に近い)
- `if let Some(x) = o { … }`、または 001 の未決事項の `if o is Some(x)` を入れるか
- `unwrap()` / `expect()` を入れるか(配列の範囲外と同じく、止まるエラーになる)
- `unwrap_or_else`、`ok()`、`or`、`filter`、`flatten` など、メソッドを足すか
- `Array<Result<T, E>>` を `Result<Array<T>, E>` にまとめる関数(`collect` に当たるもの)
- `handle` の本体と arm の中の `?` を、どの関数から返すことにするか
- 任意のフィールドに、JSX の属性に限って `T` の値をそのまま書けるようにするか(`subtitle="x"`)
- 関数の任意の引数(`fn f(x?: i32)`)と、enum のバリアントの任意のフィールド
- `pub` な関数の引数と戻り値の `Option<T>` を、`.d.ts` で `T | undefined` にするか(境界での変換になる。`@tag` の決定と一緒に)
- `extern` の JS の関数が例外を投げるとき、`-> Result<T, String>` と宣言して受け取れるようにするか
- TS から `Option` / `Result` の値を作るコンストラクタ(`@tag` の決定へ)

## 改訂履歴

- r1: 最初の草案
  - 001 r4 の決定 5(prelude の既定値)を、この提案の規則として書き直した
  - 002 から `title?: T` を引き継いだ。ただし型の等しさでは `title: Option<T>` と区別する(持ち方が違うため)
