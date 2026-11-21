# 003-option-result: 評価のまとめ

## 点の推移

| round | R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8 | mean |
|---|---|---|---|---|---|---|---|---|---|
| r1 | 4.0 | 2.7 | 3.7 | 3.7 | 3.7 | 3.0 | 3.7 | 2.7 | 3.38 |

(persona ごとの表は `python3 tools/summarize.py results/003-option-result`)

## r1 の指摘の扱い

方向(prelude、`?` は E が一致するときだけ、Rust と同じ名前のメソッド、std の Option / Result への下ろし方)は 3 人とも支持。R2 と R8 が低いのは、任意のフィールドの二重の表現(生の `T | undefined` と `{ TAG }`)と、JS の `?` の出力、001 / 002 との食い違いによる。

| 指摘 | 誰が | 扱い | r2 での変更 |
|---|---|---|---|
| JS の `?` の出力が今の js.rs(文字列で組み、複雑なら即時関数)に入らない。`&&` / `\|\|`、`while` の条件、三項の if で意味が変わる | implementer (blocker) | **採用** | 型検査の後に AST を 1 回書き換えて脱糖する(内部だけの `EK::Return`)。書ける位置を列挙する: let の右辺、文としての式、末尾の式、呼び出し / メソッド / レコード / 配列の引数(左の兄弟は一時変数)、if / match の分岐。`&&` / `\|\|` の右辺と `while` の条件は直し方つきのエラー |
| 任意のフィールドの二重の表現(`$unwrap_or` が生の値で誤る、式ごとの side table) | implementer (high), ts-dev | **要確認** | 下の決定 1 |
| `{ note?: T }` と `{ note: Option<T> }` が別の型で、注釈なしのリテラルが渡せない。省略が if の分岐の順に依存する | ts-dev (high), implementer (high), rust-dev | **要確認** | 下の決定 1 |
| 書くたびに `Some("x")` が要る | ts-dev (high) | **要確認** | 下の決定 2 |
| 総称の `{ value?: T }` の暗黙の制約が関数をまたぐ | rust-dev (high), implementer | **採用** | r2 では任意のフィールドの型に型引数を書けない(総称なら `value: Option<T>`) |
| `collect::<Result<Vec<_>,_>>()` に当たるものがない(`push` で O(n²)) | rust-dev (high) | **採用** | 組み込みの `try_map`(Result と Option)。JS は最初の Err で抜けるループ、Rust は collect |
| `ok()`、`ok_or_else`、`unwrap_or_else` がない | rust-dev | **採用** | 足す |
| `e?.f` を Rust の意味で入れるか、予約のままか | rust-dev / ts-dev(反対の意見) | **要確認** | 下の決定 3 |
| `??` | ts-dev | **要確認** | 下の決定 4 |
| `Option<Jsx>` を JSX の子に置けない(「あれば描く」) | ts-dev | **採用** | 子に置ける。None は `undefined`(何も描かない) |
| 省略したフィールドにも `note: undefined` を書く(TS 側の spread で既定値が消える) | ts-dev | **採用** | 省略したらキーを書かない(JSX の props と同じ)。明示の `None` と spread の上書きのときだけ `undefined` |
| `pipe()` の `.m(a)` の枝が postfix を通らない、`x \|> f(a)?[0]` | implementer, ts-dev, rust-dev | **採用** | pipe の右辺は `?` を読まない postfix で読み、脱糖した後に postfix のループを続ける(`.m` の枝も合流) |
| `perform Ask(q)?` だけ括弧を求めるのは不整合 | implementer | **採用** | 括弧なしで `(perform Ask(q))?` と読む |
| `?.` の予約が字句だけで、`e? .f` が通る | implementer | **採用** | 構文解析器の側で、`?` の直後の `.` を空白によらず同じに扱う |
| `handle` の中の `?` の範囲 | implementer, rust-dev | **採用** | 禁止はクロージャの境界の内側にない `?` だけ。直し方に「本体を関数に切り出す」も |
| `?` を含むクロージャの Rust(E0282) | rust-dev, implementer | **採用** | 戻り値の型を `\|x: T\| -> Result<U, E> { … }` と明示して出す |
| `.d.ts` の `Option` / `Result` がモジュールごとで衝突 | ts-dev, implementer | **採用** | `zsc-prelude.d.ts` を 1 つ出し、各モジュールは `import type`。brand も同じ |
| 三項演算子 `a ? b : c` のエラー | ts-dev | **採用** | 専用のエラー(`if` で書く) |
| `?.` のエラーが Option を返さない関数でも `(e?).f` を勧める | ts-dev | **採用** | 囲む関数の戻り値の型でメッセージを変える |
| 001 / 002 の本文との食い違い(既定値の時期、略記) | 全員 | **採用** | 001 r5 / 002 r5 で直す(進行中)。規則の持ち主は 003 |
| prelude のバリアント名と同じ名前の関数(`fn Ok`) | implementer | **採用** | 宣言の時点でエラー |
| `pub fn` が返す Result を境界で例外にするか、Option を `T \| undefined` にするか | rust-dev, ts-dev | 保留 | `@tag` の決定と一緒に、未決事項に足す |

### r1 の決定(ユーザー)

1. 任意のフィールドの型: **同じ型、境界で変換**。`note?: T` は `note: Option<T>` と同じ型(省略できる印だけが付く)。zsc の中ではいつも Option の表現で持ち、TS との境界(pub の入口と出口、JSX の props)で `undefined` と変換する。二重の表現と、書いた順への依存をなくす
2. 書くときの値: **`?:` の所だけ暗黙の Some**。期待する型が `?:` のフィールド(レコードのリテラルの値と JSX の属性の値)に限り、`T` の値を Some として受ける。変数や引数には広げない
3. `e?.f`: **予約のまま**
4. `??`: **予約のまま**(`.unwrap_or` / `.unwrap_or_else` を使う)
