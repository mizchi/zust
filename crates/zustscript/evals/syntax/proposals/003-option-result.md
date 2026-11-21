# 003: 標準の Option / Result と、`?` 演算子、任意のフィールド

状態: 評価中 (r2)

## 動機

`corpus/options.zsc` より。`examples/shapes.zsc`、`basics.zsc`、`regress.zsc` も同じ形。

1. **`Option` / `Result` をファイルごとに宣言している**
   - どのファイルも `enum Option<T> { None, Some(T) }` を書く
   - 001 の既定値(`let mut best = None` が `Option` になる)は、標準の `Option` があることを前提にしている
2. **小さな関数をファイルごとに書き直す**
   - `map_opt`、`unwrap_or` を毎回書く。ユーザーの enum にはメソッドを書けないので、関数で書くしかない
   - 呼ぶ側は `unwrap_or(map_opt(find(todos, id), (t) => t.title), "(none)")` と、内側から読むことになる
3. **エラーを渡すだけの match が、段ごとに 1 段深くなる**
   - `validate` と `rename` は、`Err(e) => Err(e)` を書くための match が半分を占める
   - 本当にしたい処理(`Ok(title) => …`)が、いちばん深い所に来る
4. **配列の各要素を検査すると、最初のエラーを手で運ぶ**
   - `titles` は、`let mut err` と `break` で最初の `Err` を覚え、ループの後でもう一度 match する
   - `out = out.push(t)` は毎回新しい配列を作るので、O(n²) になる
5. **無いかもしれない prop も、毎回渡す**
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
  - **prelude のバリアント名と同じ名前の関数は、宣言の時点でエラー**(r2)
    - 対象は 001 の「関数」と同じ範囲: fn、extern、wasm fn、ローカルの変数(引数を含む)
    - 001 の「同じ名前の関数があれば、修飾しない `Ok(…)` は関数」に従うと、`fn Ok(…)` を 1 つ足しただけで、ファイル中の `Ok(x)` が静かに関数の呼び出しになるため
    - ``error: `Ok` is a variant of the prelude's `Result`: a function cannot have this name``
- **修飾しないバリアント名の既定値**(この規則は 003 が持つ。001 はここを参照する)
  - 001 の「期待する型による解決」で決まらないとき、`None` / `Some` / `Ok` / `Err` は prelude の型になる
  - 整数リテラルが、決まらなければ i32 になるのと同じ
  - 引数のあるもの(`Some(1)`、`Ok(s)`)
    - 期待する型がなければ、その場で prelude の enum として推論する
    - 期待する型が、まだ決まっていない候補つきの型変数のときも、期待する型がないものとして扱う
    - そのために型が合わなくなったら、クイックフィックスに、候補の中で同じ名前のバリアントを持つ enum の修飾を出す(`p = Ok(s)` なら `Parsed::Ok(s)`)。候補の数で決めることはしない。直し方として示すだけ
  - 引数のない `None` は、001 のとおり関数の終わりまで遅らせ、それでも決まらなければ `Option` にする
    - ただし、決まっていない値を match にかける、フィールドを読む、メソッドを呼ぶときは、候補に prelude の enum があれば、その時点で既定値に決める(小数リテラルが f64 になるのと同じ)
  - 期待する型が、同じ名前のバリアントを持つユーザーの enum なら、そちらが勝つ(001 の `describe` の `None`)
  - prelude は固定なので、enum を足しても離れた場所の意味は変わらない
  - prelude に決まったことが原因の型エラーには、決まった場所と理由の注を付ける
    - 期待する型がなかったとき: 「期待する型がないので、ここで `Result::Ok` に決まった」
    - 使う地点で決めたとき: 「ここで `Option` に決まった(`.is_none()` のため)」
    - 例: `let x = None; x.is_none(); report(x)`(`report` は `Parsed` を取る)は、`.is_none()` の所で `Option` に決まり、`report(x)` でエラーになる。注がその場所を指す
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
  - `a.b()?.len()` のような `?.` の並びはエラー(下の「予約する字面」)
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
- **書ける位置**(列挙する。JS の出力は「出力」の節)
  - let の右辺、文としての式、ブロックと関数の末尾の式
  - 代入と複合代入の右辺、`for x in e` の `e`
  - 呼び出し、メソッド、レコード、配列、タプルの引数と要素。JSX の属性の値と子も同じ
    - 左の兄弟は先に評価して、一時変数に置く(評価順は変わらない)
  - `if` / `match` の分岐。`if` の条件と、match の照合する値
  - `&&` / `||` 以外の二項演算の両辺、単項演算、`.f` / `[i]` / `.m()` の受け手、`|>` の左
  - **書けない位置**(直し方つきのエラー)
    - `&&` / `||` の右辺: 左の値によっては評価しない式なので、先に評価する形にできない
      - ``error: `?` cannot be used on the right of `&&` / `||`: write it as an `if` (`if ok { check(d)? } else { false }`)``
    - `while` の条件: 毎回評価する式なので、ループの前に出せない
      - ``error: `?` cannot be used in a `while` condition: test it in the body (`while true { if !more()? { break; } … }`)``
- **誤りと直し方**(すべてコンパイル時)
  - E が違う
    - ``error: `?` passes on a `FormError`, but `load` returns `Result<i32, String>` ``
    - ``(no conversion is done: convert it first, e.g. `validate(d, 1).map_err(message)?`)``
  - `Result` の関数で `Option` に `?`
    - ``error: `?` on an `Option` in a function that returns `Result`: turn it into a `Result` with `.ok_or(e)?` ``
  - `Option` の関数で `Result` に `?`
    - ``error: `?` on a `Result` in a function that returns `Option`: drop the error with `.ok()?` ``
  - 戻り値が `Option` でも `Result` でもない
    - ``error: `?` can be used only in a function that returns `Option` or `Result`; `TodoRow` returns `Jsx` ``
  - LSP: E が違うときは `.map_err(…)` を足すクイックフィックス、`Option` のときは `.ok_or(…)`、`Result` のときは `.ok()` を足すクイックフィックス
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
  //                      (to stop at the first error, use `.try_map((d) => non_empty("title", d.title))`)
}
```

  - 配列の各要素を検査して最初の `Err` で止めるのは、組み込みの `try_map`(D)
- **effect(ジェネレータ)**
  - effect を持つ関数の中でも使える。JS では `function*` の中の `return` になり、`yield*` で呼んだ側に `Err` が値として返る
  - `perform` の結果にも付けられる。**括弧なしの `perform Ask(q)?` は `(perform Ask(q))?` と読む**(r2)
    - `?` だけでなく、`.len()` などの後置はすべて `perform` の結果に付く(今の parse.rs と同じ)
  - **`handle` の本体と handler の arm の中の `?` は、r2 でも書けない**
    - JS では、どちらも `$handle` に渡す別のジェネレータなので、`return` が囲む関数から返らない
    - 禁止するのは、**クロージャの境界の内側にない `?` だけ**(r2)
      - 本体や arm の中に書いたクロージャの中の `?` は、そのクロージャから返るので書ける
    - ``error: `?` cannot be used inside `handle` yet: bind the result with `let r = handle …;` and use `r?` after it, or move the body into a function that returns `Result` (`handle body_fn(…)`)``
    - どちらの関数から返るべきかは未決事項
- **`|>`**
  - `x |> f(a)?` は `(x |> f(a))?`、つまり `f(x, a)?`
    - `?` は、脱糖した後の呼び出しの結果に付く。`f(a)` の結果に付いて、それを呼ぶ、とは読まない
  - `x |> .m(a)?` も同じく `x.m(a)?`
  - **`?` の後ろの後置も、脱糖した後の結果に付く**(r2): `x |> f(a)?[0]` は `f(x, a)?[0]`、`x |> .m(a)?.len()` は `?.` の予約のエラー
  - 続けて書ける: `s |> trimmed()? |> shorter_than(80)?` は `shorter_than(trimmed(s)?, 80)?`
  - `x? |> f()` は `f(x?)`
- **`@wasm`**: Rust の `?` にそのまま下ろす(F)。E が完全に一致するので、Rust の `From` は恒等の変換になり、意味は JS と同じ
- **評価順**: `f(a(), g()?)` で `g()` が `Err` なら、`a()` は評価済み、`f` は呼ばれない(左から右。JS と Rust と同じ)
- **予約する字面**(エラーにして、意味を後で決める。r1 の決定 3 / 4 で、r2 も予約のまま)
  - `e?.f`、`e?.m()`
    - Rust の人は「`e?` してから `.f`」、TS の人は「optional chaining」と読む。意味が違うのに両方通ると、静かに読み違える
    - **空白と改行によらない**(r2): `e? .f` も、`e?` の次の行の `.f` も、同じエラー。formatter が空白を詰めても意味が変わらない
    - **メッセージは、囲む関数の戻り値の型で変える**(r2)
      - `Option` / `Result` を返す関数の中: ``error: `?.` is reserved: write `(e?).f` to return early, or `e.map((x) => x.f)` for a value that may be absent``
      - それ以外の関数の中: ``error: `?.` is reserved: write `e.map((x) => x.f)` for a value that may be absent``(`(e?).f` は書けないので勧めない)
      - LSP のクイックフィックスも同じ順(返さない関数では `.map` だけ)
  - `e ?? d`: ``error: `??` is reserved: write `e.unwrap_or(d)` (or `e.unwrap_or_else(() => d)` to compute `d` only when needed)``
  - `e??`(`Option<Option<T>>` の 2 段の `?`): `(e?)?` と書く。`??` と同じく空白によらない
  - **三項演算子** `a ? b : c`(r2): 言語にない。専用のエラーにする
    - `?` の直後に名前かリテラルが来たら: ``error: there is no `? :` operator: write `if a { b } else { c }` ``
    - `?` の後に `(` などが続き、後で `:` が来たときも、その `:` の所で同じエラー

### C. 任意のフィールド `note?: T`

```zsc
type Todo = { id: i32, title: String, note?: String, done: bool }
```

- **型**(r1 の決定 1)
  - **`note?: T` は `note: Option<T>` と同じ型**。違いは「省略できる」という印だけ
  - 読むと `Option<T>`。`t.note`、分割代入の `{ note }`、match のパターン、spread、どれも同じ
  - 型の等しさ(002 の構造的な等しさ)に、印は入らない
    - `{ note?: String }` と `{ note: Option<String> }` は同じ型。値をそのまま渡せる
    - 単一化で印が違っても、エラーにしない
  - **zsc の中では、いつも Option の表現で持つ**(`@tag` の決定に従う。今なら `{ TAG: "Some", _0: x }` / `{ TAG: "None" }`)
    - 式ごとに表現が変わることはない。どこで読んでも、ふつうの `Option` の値
  - 印が効くのは、次の 3 つだけ
    1. リテラルと JSX で省略できる(下の「省略」)
    2. 暗黙の Some(下の「書くときの値」)
    3. TS との境界での変換と `.d.ts`(下の「TS との境界」)
- **書ける場所**: レコードの型(`type` の別名、無名のレコード型、`@nominal type`)。enum のバリアントの名前付きフィールドと、関数の引数には、r2 でも書けない(未決事項)
- **型引数は書けない**(r2)
  - `type Box<T> = { value?: T }` はエラー。`value?: Array<T>` のように、型引数を含む型も同じ
  - ``error: an optional field needs a concrete type: write `value: Option<T>` for a generic field``
  - 境界の変換は、型から決まる。型引数のままだと、呼ぶ側ごとに変換が変わり、関数をまたいだ制約になるため
  - 後で緩めても、互換は壊れない
- **`T` が `Option<U>` のとき**(r2。r1 の「`T` に `Option` と `()` は書けない」はやめた)
  - `note?: Option<i32>` は書ける。型は `Option<Option<i32>>`
  - 暗黙の Some は効かない。`None` が外側の `None` か `Some(None)` かが、書いた値から決まらないため。いつも `Option<Option<i32>>` の値を書く
  - 境界で変換するのは外側だけ
    - 外側の `None` はキーなし、`Some(x)` は `x`。`x` は Option の表現のまま(今なら `Some(None)` は `{ TAG: "None" }`)
    - そのため TS 側でも、`None`(キーなし)と `Some(None)` を区別できる
  - **検査**: 境界を渡る型で、`Some(x)` の `x` が JS で `undefined` か `null` になりうるなら、その境界でエラー
    - 当たるのは `note?: ()`(`()` は JS で `undefined`)と、`@tag` の決定で `None` を `undefined` にした場合の `note?: Option<U>`
    - ``error: `note?: ()` cannot cross the TS boundary: `None` and `Some(())` would both be `undefined` (use `note: bool`)``
    - 今の表現では、`note?: Option<U>` は通る
- **省略**
  - レコードのリテラルと JSX の属性で、印のあるフィールドは省略できる。省略したら `None`
    - JSX では、`jsx()` の「足りない prop」の検査から外す
  - **期待する型が「書いた型」から来たとき**は、その場で検査する
    - 「書いた型」: 注釈と signature から、001 の「期待する型が伝わる場所」を通って届いた型
      - 例: let と引数の注釈、関数の引数と戻り値(`Ok(…)` や `?` の左を通っても同じ)、JSX の props の型、それらのフィールドの型
    - 数えないもの(推論で来た型): `if` / `match` の他の分岐、配列の他の要素、`==` / `!=` の反対側、注釈のない変数への代入の右辺(変数の型が推論で決まったもの)
    - 省略したフィールドに印がなければ、002 の「足りないフィールド」のエラー
  - **それ以外のとき**(期待する型がない、まだ型変数のまま、推論で来た)は、**候補つきの型変数にする**(r2)
    - 候補の意味: 「書いたフィールドはこの型で、ほかに印のあるフィールドがあるかもしれないレコード」
    - 001 の `None` と同じ仕組み。単一化で決まり、決まらなければ関数の終わり(finish_fn)で決める
    - レコード型 R と単一化したら、R に決まる。条件は 2 つ
      - 書いたフィールドが R にあり、型が合う
      - R にあって書いていないフィールドは、R(または、この候補が会った他のレコード型)で印がある
      - 合わなければ、002 の差分のエラー。「`note` は `Todo` では任意ではない」のように、どのフィールドかを示す
    - 候補同士は 1 つにまとめる。まとめた候補は、含むリテラルをすべて覚え、後で R に会ったら全部を R と比べる
    - 関数の終わりまで R に会わなければ、書いたフィールドだけのレコード型に決まる
      - まとめたリテラルの書いたフィールドが違えば、注釈を求めるエラー
    - 決まる前に使うとき
      - フィールドを読めるのは、書いたフィールドだけ。書いていないフィールドを読むとエラー(``annotate it, e.g. `let r: Todo = …` ``)
      - spread の元にしてよい。結果も同じ候補
    - 暗黙の Some は、候補には効かない(フィールドの型は、書いた値の型のまま)
  - 例

```zsc
let a = { id: 1, title: "a", done: false };   // 候補。後で Todo に会えば Todo、会わなければ { id, title, done }
save(a);                                      // save(t: Todo): a は Todo に決まる。note は None

todos |> .map((t) => if t.id == 0 { { id: 0, title: "a", done: false } } else { t })
// どちらの順に書いても通る。先の分岐は候補になり、t: Todo との単一化で Todo に決まる
```

  - **この方式を選んだ理由**(注釈を求める案と比べて)
    - 002 の「構造的なので、無名の値をそのまま渡せる」が、任意のフィールドを持つ型でも成り立つ
    - 書いた順(分岐の順、文の順)によらない。候補の検査は、会った型の集合で決まるため
    - 001 の `None` と同じ候補と finish_fn を使うので、仕組みが増えない
    - 注釈を求める案は、エラーが単一化の所(リテラルから離れた所)に出る。TS の人は `.map((t) => if … { { … } } else { t })` で毎回つまずく
    - 費用: 注釈のないリテラルは、関数の終わりまで型が決まらないことがある。hover は決まる前の型を `{ id: i32, title: String, done: bool, … }` と出す
- **書くときの値**(r1 の決定 2)
  - **期待する型が印のあるフィールド `note?: T` のときに限り、`T` の値を `Some` として受ける**
    - 対象は 2 つ: レコードのリテラルのフィールドの値、JSX の属性の値
    - 期待する型は「書いた型」から来たものに限る(上の「省略」と同じ。推論で来たものは数えない)
    - そのため、書いた順(分岐の順、文の順)で、受けるかどうかが変わらない
    - `{ ...t, note: "x" }` は `note: Some("x")`。`<TodoRow subtitle="x" />` も同じ
    - `Option<T>` の値(`Some(x)`、`None`、`t.note`、`Option` を返す呼び出し)は、そのまま受ける
  - 決め方: 値の式を、期待する型 `T` で検査する(Some の中身として)
    - その型が `Option<…>` に決まったら、そのまま `Option<T>` と単一化する
    - そうでなければ `T` と単一化し、`Some` で包む
    - `T` がユーザーの enum で `None` を持つとき(`kind?: Parsed`)、`{ kind: None }` は 001 の「ユーザーの enum が勝つ」で `Some(Parsed::None)` になる。無いことは、省略するか `Option::None` で書く
  - **変数と引数には広げない**
    - `let n: Option<String> = "x";` はエラー。`fn f(o: Option<String>)` に `f("x")` もエラー
    - 印のない `note: Option<String>` のフィールドにも効かない
  - `T` が `Option<U>` のときは効かない(上)
- **TS との境界**(r1 の決定 1)
  - **境界**は次の所だけ
    - `pub` な関数の入口(引数)と出口(戻り値)
    - コンポーネントの props(JSX)。`pub` でなくても境界にする。React を通って呼ばれるため
    - `extern` の関数の引数と戻り値(向きが逆: 引数は zsc → TS、戻り値は TS → zsc)
    - `pub` な `@wasm` の関数の入口と出口(TS から直接呼ばれる glue)
  - **境界でない所**: zsc の関数同士の呼び出し(`pub` な関数を zsc から呼ぶときも)、クロージャ、`@wasm` の関数を zsc から呼ぶとき
    - そのため、`pub` な関数は、変換するだけの export と、本体の内部の名前を分ける(出力)
  - **TS → zsc**
    - キーが無い、`undefined`、`null` は、どれも `None`
      - `null` は `.d.ts` の型では渡せないが、JS から来ても `None` にする
    - それ以外の値は `Some(値)`。値も、その型で同じく変換する
  - **zsc → TS**
    - `None` は、キーを書かない
    - `Some(v)` は `v`(v も、その型で同じく変換する)
    - 明示の `None` も、spread の上書き `{ ...t, note: None }` も、境界ではキーなしになる
  - **深さ**: 境界の型をたどって、印のあるフィールドに届く所すべて
    - たどるのは、レコードのフィールド、配列の要素、タプルの要素、enum のバリアントのフィールド(`Option` / `Result` の中身も)、印のあるフィールドの中身
    - 印のあるフィールドに届かない型(数値、文字列、typed array、届かないレコード)は、そのまま渡す。費用はかからない
    - 型引数の値は変換しない(総称の関数の中では中身を読まないので、そのまま返せる)
    - 関数の値は、変換する関数で包む(引数と戻り値に印のあるフィールドが届くとき)
      - 例: TS から受けた `onSave: (t: Todo) => ()` を zsc で呼ぶと、`t` を zsc → TS に変換してから渡す
  - **費用**: 届く型のオブジェクトと配列は、**境界を渡るたびに作り直す**
    - `Array<Todo>` を `pub` な関数に渡すと、配列と中のレコードを全部作り直す(長さに比例)
    - zsc のコンポーネントから zsc のコンポーネントへの props も、React を通るので、出るときと入るときの 2 回変換する
    - 包んだ関数は、境界を渡るたびに新しい関数になる(React の `memo` や依存配列では、毎回違う値に見える。未決事項)
    - 作り直したくない型には、印のない `note: Option<T>` を使う。同じ型だが、境界では変換せず、Option の表現のまま渡る
    - 境界の形は、その境界の signature に書いた印で決まる(型の等しさには入らないが、書いた注釈には残る)
  - 作り直すので、型にない余分なプロパティは落ちる(002 の未決事項。作り直さない型では残る)
- **`.d.ts`** は `readonly note?: string | undefined`
  - `?` があるので、TS 側は省略できる
  - `| undefined` は、TS から `note: undefined` と渡せるようにするため(`exactOptionalPropertyTypes` を有効にした TS でも)
  - zsc から返す値は、キーを書かない(上)。TS 側の `{ ...defaults, ...todo }` で既定値が消えない

### D. 基本のメソッド

組み込み(`Array` の `map` と同じく、検査器が知っている)にする。言語には、ユーザーの型にメソッドを定義する方法がないため。名前は Rust と同じ。

| 受け手 | メソッド | 型 |
|---|---|---|
| `Option<T>` | `map(f)` | `f: (x: T) => U` → `Option<U>` |
| | `and_then(f)` | `f: (x: T) => Option<U>` → `Option<U>` |
| | `unwrap_or(d)` | `d: T` → `T` |
| | `unwrap_or_else(f)`(r2) | `f: () => T` → `T` |
| | `ok_or(e)` | `e: E` → `Result<T, E>` |
| | `ok_or_else(f)`(r2) | `f: () => E` → `Result<T, E>` |
| | `is_some()` / `is_none()` | → `bool` |
| `Result<T, E>` | `map(f)` | `f: (x: T) => U` → `Result<U, E>` |
| | `and_then(f)` | `f: (x: T) => Result<U, E>` → `Result<U, E>`(E は同じ) |
| | `map_err(f)` | `f: (e: E) => F` → `Result<T, F>` |
| | `unwrap_or(d)` | `d: T` → `T` |
| | `unwrap_or_else(f)`(r2) | `f: (e: E) => T` → `T` |
| | `ok()`(r2) | → `Option<T>`(エラーを捨てる) |
| | `is_ok()` / `is_err()` | → `bool` |
| `Array<T>` | `try_map(f)`(r2) | `f: (x: T) => Result<U, E>` → `Result<Array<U>, E>` |
| | | `f: (x: T) => Option<U>` → `Option<Array<U>>` |

- `map_err` は、`?` の E をそろえる方法として足した(B)。これがないと、変換に毎回 match が要る
- `unwrap_or(d)` と `ok_or(e)` の引数は、いつも評価される(Rust と同じ)。無いときだけ評価したいなら `unwrap_or_else` / `ok_or_else`
- `ok()` は、`Option` を返す関数の中で `Result` に `?` を付けるときに使う(`r.ok()?`)
- **`try_map`**(r2)
  - 要素ごとに `f` を呼び、最初の `Err`(または `None`)で止めて、それを返す。全部 `Ok` なら、中身の配列を `Ok` で包む
  - `f` の戻り値の型で、Result 版か Option 版かが決まる
  - 配列を 1 回だけ作る(`for` と `push` で書くと O(n²))
- `unwrap()` / `expect()`(無いときに止める)は、r2 でも入れない(未決事項)
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

### E. JSX の子の `Option<Jsx>`(r2)

- `Option<Jsx>` の値を、JSX の子に置ける
  - `Some(j)` なら `j` を描く。`None` なら何も描かない(JS では `undefined`。React は何も描かない)
  - React の「あれば描く」(`{subtitle && <small>{subtitle}</small>}`)に当たる
- `{subtitle.map((s) => <small>{s}</small>)}` と書ける
- 子に置けるのは `Option<Jsx>` だけ。`Option<String>` などは、今までどおり型のエラー(`.map` で `Jsx` にするか、`.unwrap_or("")`)

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
  Ok({ id, title, note, done: false })        // note は Option<String>。Todo の note?: String と同じ型
}

fn titles(ds: Array<Draft>) -> Result<Array<String>, FormError> {
  ds |> .try_map((d) => non_empty("title", d.title))   // 最初の Err で止まる
}

fn rename(todos: Array<Todo>, id: i32, d: Draft) -> Result<Array<Todo>, FormError> {
  let old = find(todos, id).ok_or(NotFound { id })?;   // NotFound は ? の期待する型から FormError に決まる
  let t = validate(d, id)?;
  Ok(todos |> .map((x) => if x.id == id { { ...t, done: old.done } } else { x }))
}

fn with_note(t: Todo, s: String) -> Todo {
  { ...t, note: s }                           // 期待する型が note?: String なので、Some(s) として受ける
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
    {subtitle.map((s) => <small>{s}</small>)}   // Option<Jsx> を子に置く。None なら何も描かない
  </li>
}

pub fn TodoList({ todos }: { todos: Array<Todo> }) -> Jsx {
  <ul>{todos |> .map((t) => <TodoRow key={t.id} todo={t} subtitle={t.note} />)}</ul>
}

pub fn Placeholder() -> Jsx { <ul><TodoRow todo={{ id: 0, title: "nothing yet", done: false }} subtitle="add one" /></ul> }
//                                      ^ todo の note は省略。subtitle は String を Some として受ける
```

- `validate` と `rename` の match の入れ子がなくなり、本当にしたい処理が上から下へ並ぶ
- `titles` は、`let mut err` と `break` がなくなる
- `title_of` は、左から右へ読める
- `with_note` と `Placeholder` は、任意のフィールドに `T` の値をそのまま書く例(暗黙の Some)
- `TodoList` は `t.note`(`Option<String>`)を、任意の prop `subtitle` にそのまま渡す
- `TodoRow` は「あれば描く」を `.map` で書く

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
  // error: `?.` is reserved: write `e.map((x) => x.f)` for a value that may be absent
  // (shout は Option を返さないので、`(e?).f` は勧めない)
}

fn both(d: Draft, ok: bool) -> Result<bool, FormError> {
  Ok(ok && validate(d, 1)?.done)
  // error: `?` cannot be used on the right of `&&` / `||`: write it as an `if` (`if ok { check(d)? } else { false }`)
}

fn label(t: Todo) -> String {
  t.done ? "done" : "open"
  // error: there is no `? :` operator: write `if a { b } else { c }`
}

type Boxed<T> = { value?: T }
// error: an optional field needs a concrete type: write `value: Option<T>` for a generic field

fn note_of(s: String) -> Option<String> {
  let n: Option<String> = s;
  // error: expected `Option<String>`, found `String` (write `Some(s)`: only an optional field takes a plain value)
  n
}

fn Ok(x: i32) -> i32 { x }
// error: `Ok` is a variant of the prelude's `Result`: a function cannot have this name
```

effect、`@wasm` と一緒に使う例。

```zsc
enum Log { Info(String) -> () }
enum Ask { Ask(String) -> Result<String, FormError> }

fn save(d: Draft, id: i32) -> Result<Todo, FormError> {
  let t = validate(d, id)?;            // effect を持つ関数の中でも同じ
  perform Info("saved " + t.title);
  let n = perform Ask("note?")?;       // (perform Ask("note?"))? と読む
  Ok({ ...t, note: n })                // note?: String なので Some(n)
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
    - そのため、`undefined` を使うのは、TS との境界の任意のフィールドだけにする(C)
  - 表現を決める処理は、今と同じく js.rs の `variant_value` と、wasm.rs の `VTy::Enum` にまとめる。`?`、メソッド、境界の変換の出力も、そこから作る
- 任意のフィールドも、中ではこの表現で持つ(C)。メソッドと `?` の出力は 1 通りだけ

### JS: `?` の脱糖

- **型検査の後に、AST を 1 回書き換えて脱糖する**(r2)
  - js.rs に渡す前の 1 パス。lower.rs(Rust)は脱糖しない(Rust の `?` をそのまま使う)
  - 内部だけの `EK::Return(e)` を足す。構文には出さない(parser は作らない)
  - `?` を含む文を、一時変数の let の並びにする。`?` は `match` と `EK::Return` になる
  - 式の位置にある `if` / `match` で、分岐に `?` を含むものは、`let $t = if …;` の文に出す(js.rs の今の `Dest::Assign` の形)
  - 書き換えた後、`EK::Return` は文の位置にしかない。js.rs はそれを `return e;` と書くだけ
  - 即時関数(`iife()`)の中には `EK::Return` を置かない。置いたら内部のエラー(`loop_exit` と同じ検査)

```zsc
// 書き換えの前
let t = non_empty("title", d.title)?;
// 書き換えの後(内部の AST。«return» は EK::Return)
let $t1 = non_empty("title", d.title);
let t = match $t1 { Err(_) => «return $t1», Ok($v1) => $v1 };
```

```js
// let t = non_empty("title", d.title)?;
const $t1 = non_empty("title", d.title);
if ($t1.TAG === "Err") return $t1;     // Err の値を作り直さずに返す(E が同じなので、同じ形)
const t = $t1._0;

// Option の ?: let old = find(todos, id)?;
const $t2 = find(todos, id);
if ($t2.TAG === "None") return $t2;
const old = $t2._0;

// effect を持つ関数(function*)の中: 呼び出しが yield* になるだけ
const $t3 = yield* validate(d, id);
if ($t3.TAG === "Err") return $t3;

// 式の途中の ?: f(a(), g()?)。左の a() を先に一時変数に置き、評価順を保つ
const $t4 = a();
const $t5 = g();
if ($t5.TAG === "Err") return $t5;
return f($t4, $t5._0);

// 分岐の中の ?: f(a(), if c { g()? } else { 0 })。if を文に出す(三項演算子にしない)
const $t6 = a();
let $t7;
if (c) {
  const $t8 = g();
  if ($t8.TAG === "Err") return $t8;
  $t7 = $t8._0;
} else {
  $t7 = 0;
}
return f($t6, $t7);
```

- 左の兄弟が変数かリテラルなら、一時変数にしなくてよい(zsc では、`g()` がローカルの変数を書き換えられないため)
- クロージャの中の `?` は、アロー関数の `return` になる(クロージャから返る)。書き換えは、クロージャの本体を 1 つの関数として扱う

### JS: メソッド

- 小さな実行時ヘルパにする(`$handle` と同じ置き方。使うものだけ書き出す)

```js
// find(todos, id) |> .map((t) => t.title) |> .unwrap_or("(none)")
$unwrap_or($opt_map(find(todos, id), (t) => t.title), "(none)")

function $opt_map(o, f) { return o.TAG === "Some" ? { TAG: "Some", _0: f(o._0) } : o; }
function $unwrap_or(o, d) { return o.TAG === "Some" || o.TAG === "Ok" ? o._0 : d; }

// ds |> .try_map((d) => non_empty("title", d.title))
function $try_map_r(xs, f) {
  const out = [];
  for (const x of xs) { const r = f(x); if (r.TAG === "Err") return r; out.push(r._0); }
  return { TAG: "Ok", _0: out };
}
```

- Option 版の `try_map` は `$try_map_o`(`None` で止め、`Some` で包む)

### JS: 任意のフィールド

```js
// Ok({ id, title, note, done: false })      note は Option<String> の変数。中では Option のまま
return { TAG: "Ok", _0: { id, title, note, done: false } };
// { id: 0, title: "nothing yet", done: false }   省略したフィールドは、中では None
({ id: 0, title: "nothing yet", note: { TAG: "None" }, done: false })
// { ...t, note: s }   暗黙の Some
({ ...t, note: { TAG: "Some", _0: s } })
```

- **境界の変換**は、型ごとの小さな関数にする(使う型だけ書き出す。名前は内部だけなので番号でよい)

```js
// TS → zsc(Todo)
function __zs_in_1(x) {
  return { id: x.id, title: x.title, note: x.note == null ? { TAG: "None" } : { TAG: "Some", _0: x.note }, done: x.done };
}
// zsc → TS(Todo)。None ならキーを書かない
function __zs_out_1(x) {
  const o = { id: x.id, title: x.title, done: x.done };
  if (x.note.TAG === "Some") o.note = x.note._0;
  return o;
}

// pub fn title_of(todos: Array<Todo>, id: i32) -> String
// export は変換するだけ。本体は内部の名前。zsc からの呼び出しは __zs_title_of を呼ぶ
export function title_of(todos, id) { return __zs_title_of(todos.map(__zs_in_1), id); }
function __zs_title_of(todos, id) { … }

// コンポーネント: props は pub でなくても入口で変換する
export function TodoRow(props) { const { todo, subtitle } = __zs_in_2(props); … }
// <TodoRow todo={t} subtitle={t.note} />   出る props も変換する
_jsx(TodoRow, __zs_out_2({ todo: t, subtitle: t.note }))
// <TodoRow todo={x} subtitle="add one" />   値が決まっている所は、その場で書く
_jsx(TodoRow, { todo: __zs_out_1(x), subtitle: "add one" })
// <TodoRow todo={x} />   省略した prop は書かない
_jsx(TodoRow, { todo: __zs_out_1(x) })
// {subtitle.map((s) => <small>{s}</small>)}   Option<Jsx> の子
$opt_child($opt_map(subtitle, (s) => _jsx("small", { children: s })))
function $opt_child(o) { return o.TAG === "Some" ? o._0 : undefined; }
```

- 境界の型に印のあるフィールドが届かなければ、変換の関数も内部の名前も作らない(今と同じ出力)
- `extern` の関数は、向きが逆になるだけ(引数に `__zs_out_…`、戻り値に `__zs_in_…`)

### `.d.ts`

```ts
// zsc-prelude.d.ts(1 つだけ出す)
export type Option<T> = { readonly TAG: "None" } | { readonly TAG: "Some"; readonly _0: T };
export type Result<T, E> = { readonly TAG: "Ok"; readonly _0: T } | { readonly TAG: "Err"; readonly _0: E };
export type i32 = number & { __zsc_num: "i32" };   // 数値の brand もここに移す(今の形のまま)
export declare function i32(x: number): i32;

// todos.d.ts
import type { Option, Result, i32 } from "./zsc-prelude";
export type Todo = { readonly id: i32; readonly title: string; readonly note?: string | undefined; readonly done: boolean };
export declare function TodoRow(props: { readonly todo: Todo; readonly subtitle?: string | undefined }): $ReactElement;
export declare function title_of(todos: readonly Todo[], id: i32): string;
```

- **`Option` / `Result` と数値の brand は、`zsc-prelude.d.ts` に 1 回だけ書く**(r2)
  - 各モジュールの `.d.ts` は `import type` で参照する。再 export はしない
  - brand の TS 向けのコンストラクタ(`i32(x)`)も、各モジュールの JS から `zsc-prelude.js` に移す(同じ名前の export が並ばないように)
  - TS 側で `export * from "./a"; export * from "./b"` と並べても衝突しない(TS2308)。TS の既存の `Option`(fp-ts など)とも、import の名前で分けられる
  - 中身は、今ユーザーが宣言したときと同じ。移行しても TS 側の型は変わらない
  - 形は `@tag` の決定で変わりうる(001 の namespace `Option.Some` も同じ)
- `note?: Option<U>` は `readonly note?: Option<U> | undefined`
- TS から `Option` の値を作るコンストラクタは、001 と同じく `@tag` の決定で扱う

### Rust / wasm(`@wasm`)

- `Option<T>` / `Result<T, E>` は、Rust の標準の `Option` / `Result` に下ろす(`ZsOption` は作らない)
  - T と E が Copy なら Copy になるので、今の「レコードと enum は Copy で値渡し」と同じに使える
  - パターンとバリアントは `Some(x)` / `None` / `Ok(x)` / `Err(e)`。ユーザーの enum の `None` は、今までどおり `ZsParsed::None` と修飾して出すので衝突しない
- `?` は Rust の `?`。メソッドは Rust の同じ名前のメソッド(`map`、`and_then`、`unwrap_or`、`unwrap_or_else`、`ok_or`、`ok_or_else`、`ok`、`map_err`、`is_some` …)
  - `map` / `and_then` / `map_err` などのクロージャは、Rust のクロージャになる(今は配列のメソッドの引数だけなので、ここに足す)
  - `try_map` は `xs.iter().map(f).collect::<Result<Vec<_>, _>>()`(Option 版は `Option<Vec<_>>`)
- **`?` を含むクロージャは、引数と戻り値の型を書いて出す**(r2): `|x: i32| -> Option<i32> { … }`
  - Rust の `?` は `From::from` を通るので、型を書かないと rustc がエラーの型を決められない(E0282)
  - 注釈のない `let r = Ok(1);` も、検査器が決めた型を書く: `let r: Result<i32, ZsE> = Ok(1);`
- 任意のフィールドは、Rust ではただの `Option<T>` のフィールド(印は Rust に出ない)
  - struct の名前は、印によらない(002 の E)。`{ x?: f64 }` と `{ x: Option<f64> }` は同じ struct
- 境界
  - スロットへの分解は、今の `VTy::Enum` のまま。タグの番号は宣言の順(`None` = 0、`Some` = 1、`Ok` = 0、`Err` = 1)で、今の `ZsOption` と同じ
  - 組み立て直すときのパスだけが変わる(`ZsOption::Some(..)` → `Some(..)`)
  - 任意のフィールドも、中では Option の表現なので、同じ `VTy::Enum` で分解する。新しい VTy は要らない
  - `pub` な `@wasm` の関数は、TS から呼ばれる glue の前で、C の変換をする(`pub fn` と同じ)
- `@wasm` の部分集合には文字列がないので、`Result<T, String>` は `@wasm` では使えない。エラーは数値やデータなしの enum で表す

```rust
fn zs_safe_div(a: i32, b: i32) -> Option<i32> { if b == 0 { None } else { Some(a.wrapping_div(b)) } }
fn zs_ratio(a: i32, b: i32, c: i32) -> Option<i32> {
    let x = zs_safe_div(a, b)?;
    zs_safe_div(x, c)
}
// xs.map((x) => safe_div(x, c)?) の形のクロージャ
|x: i32| -> Option<i32> { let y = zs_safe_div(x, c)?; Some(y) }
```

## 実装の注意

- **prelude**: check.rs の enum の表に、ユーザーの宣言より先に `Option` / `Result` を入れる。`prelude` の印を持たせる
  - js.rs と dts は、使われたものだけ `zsc-prelude.d.ts` に書き出す。lower.rs は定義を書かず、`rs_type` を `Option` / `Result` にする
  - 既定値の規則は、001 の finish_fn の第 1 段(候補に prelude の enum があればそれに決める)のまま
  - fn / extern / wasm fn / ローカルの宣言で、名前が `None` / `Some` / `Ok` / `Err` ならエラー
- **構文**
  - lexer の `PUNCTS` に `?` だけを足す。`?.` と `??` は字句にしない
  - postfix() で、後置の `?` を `EK::Try(e)` として読む
    - 直後のトークンが `.` か `?` なら、予約のエラー。トークンの間の空白と改行は見ない
    - 直後が名前かリテラルなら、三項演算子のエラー
  - pipe() は、右辺を「`?` を読まない postfix」で読み、脱糖した呼び出し(`.m` の枝も)を作ってから、その上で postfix のループ(`?`、`[i]`、`.f`、`.m()`)を続ける
    - 今の pipe() は、`.` の枝で `.m(args)` を読んですぐ返り、`_` の枝で `Call(Try(f(a)), [x])` を作ってしまう。どちらも 1 つのループに合流させる
  - `perform Ask(q)` の後の postfix は、今のまま全体に付く(`?` も同じ)
  - 型の位置のレコードで、`ident ?:` を任意のフィールドとして読む
- **型検査**
  - 関数とクロージャの戻り値の型を、スタックで持つ(`EK::Closure` で積む)。`EK::Try` はその先頭を見る
  - `EK::Try` の左の式には、戻り値の型から作った `Result<_, E>` / `Option<_>` を期待する型として渡す
  - `handle` の本体と arm の中にいるかの印を持つ。クロージャに入るたびに外す(`closure_bases` と同じ)。印があるときの `EK::Try` はエラー
  - `EK::Try` が `&&` / `||` の右辺、`while` の条件にあればエラー(構文の位置で決まるので、検査の前に見てもよい)
  - method_sig に `Option` / `Result` / `Array::try_map` の行を足す。期待する型を戻り値の型と先に単一化してから、引数を検査する
  - **任意のフィールド**
    - 002 の `Ty::Rec` のフィールドに `optional: bool` を持たせる。等しさには入れない(002 の `Ty::Fn` の引数名と同じく、`derive(PartialEq)` に混ざらない別のフィールド)。読むときの型は `Option<T>`
    - 任意のフィールドの型に `Ty::Param` を含めばエラー
    - 期待する型に「書いた型から来た」かの印を付ける。注釈と signature で付け、伝わる場所を通るときは引き継ぐ。`EK::If` の `infer(el, Some(&tt))` のように、他の分岐から渡す期待する型では外す。配列の他の要素、`==` の反対側、注釈のない変数への代入でも外す
    - レコードのリテラル: 期待する型が書いた型から来た `Ty::Rec` なら、その場で省略と暗黙の Some を検査する。そうでなければ候補つきの型変数にする(001 の候補と同じ表に、「書いたフィールド」とリテラルの Id の並びを持たせる)
    - 単一化: 候補と `Ty::Rec` が会ったら C の条件で決める。候補同士はまとめる
    - finish_fn の第 1 段で、残ったレコードの候補を、書いたフィールドだけの型に決める
    - side table: 省略したフィールド(リテラルの Id → フィールド名)と、暗黙の Some(値の式の Id)。js.rs と lower.rs は、None を足す所と Some で包む所を、これで知る
    - これは書いた値を作る所の表だけで、読む所の表現は変えない(r1 の「式ごとの表現」の side table はなくなる)
  - jsx() の「足りない prop」の検査と、レコードのリテラルの「足りないフィールド」の検査から、印のあるフィールドを外す
  - `Option<Jsx>` を JSX の子の型として受ける
- **`?` の脱糖**(js.rs の前の 1 パス)
  - 文ごとに、`?` を含むかを先に調べる(`loop_exit` と同じ形の関数)
  - 含む文は、左から右へ部分式を一時変数の let に出し、`?` を `match` と `EK::Return` にする
  - 式の位置の `if` / `match` で `?` を含むものは、let の文に出す
  - js.rs は `EK::Return` を `return` と書く。`complex()` は `EK::Return` を含む式を文にする
- **JS の出力**
  - 境界の変換の関数を、型ごとに 1 つ作る(印のあるフィールドに届く型だけ)。`pub` な関数とコンポーネントは、export と内部の名前を分ける
  - `fn_name` は、zsc の中からの呼び出しに内部の名前を返す
  - JSX の props は、コンポーネントの props の型の `__zs_out_…` を通す。値が決まっている所(省略、リテラルの値)は、その場で書く
- **Rust の出力**: lower.rs の `closure()` は、`?` を含むクロージャに、検査器が決めた引数と戻り値の型を書く
- **effect の推論**: EffWalk は `EK::Try` の中を見るだけ(`?` は effect を足さない)
- 解決はすべて関数の中で閉じる。アイテム単位の増分検査は変わらない

## 代替案

1. **TS 風の nullable(`T | null` / `T | undefined`)を言語の型にする**
   - TS の人には近いが、`Option<Option<T>>` を区別できない。総称の関数で型を消すと、`T` 自体が null のときと区別できない
   - 失敗の理由を持てない(`Result` の代わりにならない)
   - `@wasm` でも、Rust の `Option` に素直に下ろせない
   - 選ばない。`undefined` は、TS との境界の任意のフィールドにだけ使う
2. **TS の `?.` と `??` を主な道具にする**
   - nullable の型が前提になる(1 と同じ問題)
   - 後置の `?` と字面が重なる。そのため両方を予約した(r1 の決定 3 / 4)
3. **`?` を例外で書き出す**(`$try(e)` が投げ、関数の入口の `try` で受ける)
   - 即時関数の中でも、評価順を気にせずに書ける
   - ただし失敗のたびに例外になり遅い。出力も読みにくく、ジェネレータの中での振る舞いも追いにくい
   - 選ばない。`return` で書き出し、即時関数を避ける
4. **`?` を js.rs の文字列の組み立ての中で、その場で持ち上げる**(r1 の書き方)
   - Call / Method / Bin / Record / Array / JSX / if のすべてに持ち上げの処理が要り、`&&` / `||` や `while` の条件で意味が変わる
   - 選ばない。型検査の後に AST を 1 回書き換え、書ける位置を列挙する
5. **`From` のような、エラーの型の自動の変換**
   - trait がない。暗黙の変換をしない方針とも合わない
   - 選ばない。`map_err` を明示して書く
6. **任意のフィールドを、中では `T | undefined` で持つ**(r1 の案)
   - 境界で作り直さずに済む
   - ただし同じ `Option<T>` の型に 2 つの表現ができる。メソッド、match、`?` が、受け手が任意のフィールドを直接読んだ式かで出力を変えることになり、`$unwrap_or` のような共通のヘルパが誤る
   - `{ note?: T }` と `{ note: Option<T> }` を別の型にする必要があり、注釈のないリテラルが渡せない
   - r2 では選ばない(r1 の決定 1)。中では Option の表現で持ち、境界で作り直す
7. **`note?: T` と `note: Option<T>` を、境界の形まで同じにする**(印を境界でも見ない)
   - `.d.ts` が `readonly note: Option<string>` になり、TS から省略できない
   - 選ばない。印は、省略、暗黙の Some、境界の 3 つにだけ効かせる
8. **注釈のないリテラルで省略したら、注釈を求めるエラーにする**
   - 実装は小さい
   - ただしエラーがリテラルから離れた単一化の所に出る。分岐の中のリテラルのように、注釈を書く場所がない所も多い
   - 選ばない。候補つきの型変数にする(C)
9. **暗黙の Some を、変数と引数にも広げる**
   - `f("x")` と書ける
   - 期待する型で値の意味が変わる所が増え、総称の呼び出しや単一化の順で結果が変わりうる
   - 選ばない(r1 の決定 2)。印のあるフィールドの、リテラルの値と JSX の属性の値だけ
10. **メソッドを、prelude のソースに関数として書く**(`fn map<T, U>(o: Option<T>, …)`)
    - ユーザーの型にメソッドを書く方法がないので、`o.map(…)` と書けない。名前も利用者の関数(`regress.zsc` の `unwrap_or`)とぶつかる
    - 選ばない。組み込みにする
11. **`try { … }` ブロックや do 記法**: `?` の範囲を関数より狭くできるが、覚えることが増える。選ばない

## 未決事項

- `?.` と `??` の意味(r2 も予約のまま。r1 の決定 3 / 4)
  - `?.`: (a) Rust の意味(`(e?).f`)、(b) TS の optional chaining(`e.map((x) => x.f)` の略記)、(c) 予約のまま
  - `??`: `unwrap_or_else` の略記(右辺を遅らせて評価する)として入れるか
- `if let Some(x) = o { … }`、または 001 の未決事項の `if o is Some(x)` を入れるか
- `unwrap()` / `expect()` を入れるか(配列の範囲外と同じく、止まるエラーになる)
- `or`、`or_else`、`filter`、`flatten` など、ほかのメソッドを足すか
- `handle` の本体と arm の中の `?` を、どの関数から返すことにするか
- 関数の任意の引数(`fn f(x?: i32)`)と、enum のバリアントの任意のフィールド
- **`pub` な関数の境界の `Option` / `Result`**(r1 の保留。`@tag` の決定と一緒に)
  - 引数と戻り値の `Option<T>` を、`.d.ts` で `T | undefined` にするか(任意のフィールドと同じ変換になる)
  - `pub fn` が返す `Result` を、境界で JS の例外に変えるか(wasm_bindgen の `Result<T, JsError>` と同じ。`@throws` のような印)
- 境界で包んだ関数が、渡るたびに新しい関数になること(React の `memo` と依存配列)。同じ関数には同じ包みを返す表を持つか
- 境界で作り直すとき、型にない余分なプロパティが落ちること(002 の未決事項と一緒に)
- `extern` の JS の関数が例外を投げるとき、`-> Result<T, String>` と宣言して受け取れるようにするか
- TS から `Option` / `Result` の値を作るコンストラクタ(`@tag` の決定へ)

## 改訂履歴

- r2(r1 の指摘と、決定事項から。summary.md の表を参照)
  - 任意のフィールド(決定 1)
    - `note?: T` を `note: Option<T>` と同じ型にした。違いは省略できる印だけで、型の等しさに入れない
    - 中ではいつも Option の表現で持ち、TS との境界(`pub` の入口と出口、コンポーネントの props、`extern`、`pub` な `@wasm`)で `undefined` と変換する。変換の深さと、作り直す費用を書いた
    - 省略したフィールドはキーを書かない、という r1 の指摘の採用は、境界の出力の規則になった(中では None を持つので、spread の上書きもそのまま)
    - 注釈のないリテラルの省略は、候補つきの型変数にし、001 と同じく finish_fn で決めることにした。他の分岐から来た期待する型は、その場の検査に数えない
    - 「`T` に `Option` と `()` は書けない」をやめ、境界で `undefined` と重なる型(`note?: ()` など)だけをエラーにした
    - 任意のフィールドの型に型引数を書けないことにした
    - `.d.ts` は `note?: T | undefined` のまま
  - 印のあるフィールドのリテラルの値と JSX の属性の値に限り、`T` の値を Some として受けることにした(決定 2)。`with_note` と `Placeholder` の例を直した
  - `?.` と `??` は予約のまま(決定 3 / 4)
  - `?` の JS の出力: 型検査の後に AST を 1 回書き換える脱糖(内部の `EK::Return`)にした。書ける位置を列挙し、`&&` / `||` の右辺と `while` の条件は直し方つきのエラーにした。分岐の中の `?` の JS を足した
  - メソッド: `ok()`、`ok_or_else`、`unwrap_or_else`、`Array::try_map` を足した。`titles` の例を足し、corpus にも変更前の `titles` を足した
  - `Option<Jsx>` を JSX の子に置けるようにした(E)。`TodoRow` の例を直した
  - 構文: pipe の右辺の後置を、脱糖した後に続けることにした。`perform Ask(q)?` を括弧なしで読むことにした。`?.` の予約を空白によらないものにした。三項演算子の専用のエラーを足した
  - `?.` のエラーを、囲む関数の戻り値の型で変えることにした
  - `handle` の中の `?` の禁止を、クロージャの境界の内側にないものだけにした。直し方に「本体を関数に切り出す」を足した
  - prelude のバリアント名と同じ名前の関数を、宣言の時点でエラーにした
  - 既定値の規則は 003 が持ち、001 はここを参照することにした。使う地点で決めたときの注を書いた
  - Rust: `?` を含むクロージャに、引数と戻り値の型を書いて出すことにした。任意のフィールドの struct の名前は印によらない(002 の E)
  - `.d.ts`: `Option` / `Result` と数値の brand を `zsc-prelude.d.ts` に 1 回だけ書くことにした
  - 代替案: r1 の書き方(js.rs の中での持ち上げ、中で `T | undefined` で持つ)、注釈を求める案、暗黙の Some を広げる案を足した
  - 未決事項: 決まったもの(メソッド、`collect` に当たるもの、JSX の属性の暗黙の Some)を外した。`pub` な関数の Result を例外にするか、包んだ関数の同一性、余分なプロパティを足した
- r1: 最初の草案
  - 001 r4 の決定 5(prelude の既定値)を、この提案の規則として書き直した
  - 002 から `title?: T` を引き継いだ。ただし型の等しさでは `title: Option<T>` と区別する(持ち方が違うため)
