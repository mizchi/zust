# アプリケーションのコードの形のベンチマーク

Date: 2026-11-20

KPI が測っていたプログラムは、種類が少なかった。

- `bench/gen.py`: 整数の計算をする 5000 個の関数。
- `bench/gen_traits.py`: 総称の trait とイテレータの連鎖。
- cratebench: 小さなクレート 7 つ（fnv、itoa、hex など。ハッシュ関数と数の書式）。
- `bench/rt`: 実行時間を測る小さなプログラム 7 本。

どれにも、ほとんどのアプリケーションを形作る次のようなコードがない。

- データを持つ enum と、それに対する match
- 文字列の処理
- `?` によるエラー処理
- trait object
- 借用する構造体
- `macro_rules!`
- モジュールの木

これらのどれかを遅くする変更があっても、KPI には出なかった。

## パターン

`bench/gen_patterns.py PATTERN N [variant]` は、9 つの形のどれかのプログラムを書き出す。どれも正しい Rust（edition
2021）で、main がグループごとに 1 つのチェックサムを出すので、rustc の出力と比べられる。N はグループの数で、パターンの
コードを名前を変えて繰り返すので、プログラムは線形に大きくなる。`variant` は最初のグループの定数を 1 つ変える。これが
KPI の「1 関数の編集」の編集である。

| パターン | 代表するコード | 使うもの |
|---|---|---|
| `interp` | コンパイラ、インタプリタ、クエリエンジン | 文字の上の字句解析、Box を使う enum の AST への再帰下降の構文解析、網羅的な match による評価 |
| `records` | CLI ツール、ログや CSV の処理 | 行を分けて構造体に読み込む（Option、`?`）、`entry().or_default()` を使う HashMap、`sort_by`、`format!` |
| `errors` | サービス、リクエストの処理 | Display と From を持つエラーの enum、Result と `?` による検証、Option のコンビネータ |
| `state` | プロトコル、ゲーム、ワークフローエンジン | データを持つ enum、ガードつきの `match (state, event)`、`..` のパターン |
| `plugins` | GUI やアプリケーションのフレームワーク、ミドルウェア | `Vec<Box<dyn Trait>>`、既定のメソッド、`Rc<RefCell<..>>` で共有する文脈、Option への `?` |
| `borrows` | 構文解析器、プロトコルのデコーダ | `&'a str` を持つ構造体、入力の一部を返すメソッド、`&mut` での走査 |
| `macros` | コード生成の多いクレート（bitflags 風） | 構造体・impl・表を生成する `macro_rules!`（繰り返しと再帰） |
| `modules` | 大きなアプリケーションの構成 | 入れ子のモジュール、`pub(crate)` / `pub(super)`、`pub use`、`use crate::..`、static、モジュールをまたぐ impl |
| `generics` | ライブラリのコード | いくつもの型で使う総称のコンテナ、タプル・Vec・Option に実装した trait、`IntoIterator` の境界、手書きのイテレータのアダプタ |

`mix` は、全パターンを N グループずつ 1 つのプログラムに入れる。アプリケーションのクレートの形をした 1 本のプログラムである。

## 測り方

- `python3 bench/patterns.py [--n 4] [--instructions]` は、パターンを 1 つずつと `mix` を測る。
  `target/release/zustc` を使うので、リポジトリの根で `cargo build --release` してから実行する。
  - それぞれのパターンについて、zustc と rustc でビルドし、出力を比べる。rustc が PATH にある必要がある。
    `--no-rustc` を付けると比べないので、そのときは成功しても何も確かめていない。
  - そのうえで、コールドビルド・`check`・1 関数を編集した後のビルドの時間の中央値を出す。`--instructions` を付けると、
    `check` と編集後のビルドを cachegrind で数えた命令数も出す。
  - 終了コードが 1 になるのは、パターンかその編集がビルドできないとき、プログラムがエラーで終わるとき、出力が違うときである。
  - 書き出すのは表（と `--json`）だけで、KPI のログに行を足すのは `bench/kpi_track.py` である。
- `bench/kpi_track.py`（つまり pull request ごとの `KPI` ワークフロー）に、2 つの KPI を足した。`patterns check` と
  `patterns edit` で、`mix 4`（2600 行）を測る。
- `tests/bench_patterns.rs` は、すべてのパターンを N = 2 で、`mix` を編集つきでビルドし、出力が rustc のものと同じかを
  確かめる。zustc がビルドできなくなったパターンがあれば、`cargo test` が失敗する。これは CI のように python3 と rustc が
  PATH にある場合で、ないときはテストがそう出して通る。既定の `acg` feature に含まれる。

N = 4 のパターン（2026-11-20、この機械）:

| パターン | 行 | rustc と同じ | コールドビルド ms | check ms | 1 関数の編集 ms | check M 命令 | 編集 M 命令 |
|---|---:|---:|---:|---:|---:|---:|---:|
| interp | 560 | yes | 54 | 40 | 42 | 133.2 | 53.0 |
| records | 212 | yes | 65 | 38 | 39 | 156.2 | 57.7 |
| errors | 292 | yes | 54 | 28 | 35 | 117.1 | 46.4 |
| state | 260 | yes | 41 | 15 | 28 | 64.9 | 26.0 |
| plugins | 320 | yes | 66 | 47 | 34 | 134.9 | 51.2 |
| borrows | 252 | yes | 47 | 28 | 34 | 102.7 | 43.8 |
| macros | 208 | yes | 39 | 22 | 35 | 86.0 | 41.6 |
| modules | 256 | yes | 46 | 23 | 34 | 84.0 | 36.2 |
| generics | 268 | yes | 60 | 39 | 43 | 134.0 | 56.8 |
| mix | 2596 | yes | 199 | 144 | 77 | 669.2 | 240.0 |

行あたりの検査がいちばん重いのは `records` で、1 行あたり 0.74 M 命令である（`interp` は 0.24）。このコードは format!、
HashMap の entry API、イテレータの上のクロージャを通り、どれも prelude のコードに展開される。

## 書いてみて zustc に見つかったこと

パターンは最初、zustc が対応しているかを気にせず、rustc がビルドする Rust として書いた。問題が 6 つ出た。そのうち 4 つを
ここで直し、それぞれに `tests/probe` のテストを足した（期待する出力は rustc のもの）。

1. **直した:** モジュールのパスで指した static でメソッドを呼ぶと（`config::NAMES.len()`、`config::NAMES[i].len()`、
   `super::config::NAMES[i]`）、"no method `len` on type `str`" になった。式の型をコンパイルの前に出す `peek_syn` が、
   そういうパスを読まなかったので、違う型の上でメソッドを探していた。今は、`expr` と `place` がすでにそうしていたように、
   パスが指す static を番号で読む（`crates/typeck/src/acg/mod.rs`）。`expr` と `place` は読んでいたが、名前だけで
   引いていたので、同じ名前のローカル変数（`m::VALUES` のそばの `let VALUES = ..`）のほうを読んでいた。これらも番号で
   引くようにした。
   テスト: `tests/probe/static_path_method.rs`。
2. **直した:** `I: IntoIterator<Item = A>` でしか決まらない型引数が、引数が Vec か HashMap のときに推論できなかった
   （"cannot infer type parameter `A`"）。prelude のコレクションは IntoIterator の impl ではなく `into_iter` メソッドを
   持つので、読める `Item` がなかった。今は、`into_iter` が返すイテレータの Item を使う（`solve_assoc_bounds`、
   `crates/typeck/src/acg/p1_expr.rs`）。配列の引数は対象外。
   テスト: `tests/probe/into_iterator_item.rs`。
3. **直した（誤ったコードだった）:** 2 つのモジュールが同じ名前の trait をそれぞれ定義し、同じ型に実装すると、1 つの impl を
   共有していた。`mod a { trait M { .. } impl M for u32 { .. 1 } }` と `mod b { trait M { .. } impl M for u32 { .. 2 } }`
   では、どちらのモジュールも最初の impl を呼び、rustc が `5 10` を出すところ zustc は `5 5` を出していた。プログラムが
   ビルドできてしまうので、6 つの中でいちばん重かった。
   - モジュールを平らにすると trait の名前は変わる（`a__M`、`b__M`）が、メソッド呼び出しは trait を名指ししない。どの
     impl を指すかは呼び出しの場所で見える trait で決まるのに、平らにするとモジュールの情報が失われる。
   - `lexer::trait_scopes` が、平らにする前のソースから、モジュールごとに見える trait を読むようにした。そこで定義した
     もの、および `use`・`use .. as`・glob・再輸出（あるモジュールの `pub use` を、別のモジュールが取り込む）で取り込んだ
     ものである。読むのは、2 つのスコープが同じ名前の trait を定義しているときだけで、それ以外はテキストを軽く見るだけで
     終わり、字句解析はしない。
   - 同じ名前のメソッドの impl から 1 つを選ぶ `pick_overload` は、呼び出す関数のアイテムのソースの行で見える trait の
     impl を選ぶ。
   - rustc と同じく、合う固有メソッドはどの trait のメソッドより先で、モジュールが定義するか名前で取り込んだ名前は、glob
     から来る同じ名前を隠す。アイテムで見える trait はそのキャッシュのキーに入るので、`use` を変えればビルドし直す。
     （そのアイテムが別のアイテムにインライン展開されるときも）。trait を名指しした呼び出し（`crate::a::M::m(&x)`）は、
     その trait の impl を取る。テスト: `tests/probe/same_trait_precedence.rs`、`tests/probe/same_trait_explicit.rs`。
   - 扱わないもの: スコープはモジュールで、ブロックではないので、1 つの関数の 2 つのブロックがそれぞれ違う `M` を `use`
     すると、両方が見える。1 行に書いた 2 つのモジュールは行を共有する。依存するクレートのあるビルドは前のまま（最初の
     impl）で、それらの行はそのクレートの行ではないためである。
4. **直した:** クロージャは、それがある型とメソッドの名前で名付けられ（`Vec::m::{closure#0}`）、trait の名前を含まない。
   Vec への impl がどちらも、クロージャを含むメソッド `m` を持つ 2 つの trait があると、"duplicate definition of
   `Vec::m::{closure#0}::call`" になっていた。
   - 2 つ目のそういうクロージャは、名前と、その `call` の型と、シンボルに、アイテムの番号を付けるようにした。シンボルは
     `<関数のシンボル>.c<k>` で、2 つで同じだったので、片方のクロージャのコードが両方に使われていた。
   - 名前を変えるのは、前はエラーになっていたプログラムだけである。
   - 3 と 4 のテスト: `tests/probe/same_trait_two_modules.rs`。モジュールで定義した trait、`use super::M`、
     `use crate::b::M as BM`、`use crate::a::*`、ルートの `use`、両方の impl の中のクロージャを含む。`-O` でも minclif でも
     通る。`tests/probe/same_trait_reexport.rs` は、facade を通した再輸出（名前と glob）、`trait` と名前の間の改行、両方の
     モジュールが共有する型に `impl Fn` を返すメソッドを足す（そのクロージャはメソッドの戻り値の型に名前が出るので、名前の
     付け替えはそこも直す）。
5. **未解決:** `use super::model;` の後の `model::make()` は、クレートのどこかに `model` という名前のモジュールがもう 1 つ
   あると、"cannot find value (unsupported path)" になる。`use super::model::make;` と `super::model::make()` は通る。
6. **未解決:** trait のメソッドを値として渡すこと（`.map(Tr::m)`、`.map(ToString::to_string)`）は、
   "cannot find value (unsupported path)" になる。

これらを切り分ける途中で、次も見つかった。`fn f<I: IntoIterator>(a: I) -> Option<I::Item>` で zustc が panic する
（`crates/syntax/src/types_enc.rs:11`）。また、`IntoIterator` の境界のところに Range を渡せない。

生成器は 5 と 6 を避けて書いてあるので、パターンは今すぐ測れる。避け方はそれぞれ `bench/gen_patterns.py` に注記した。

- `generics` は `Tr::m` ではなくクロージャを渡す。
- `modules` はモジュールではなくアイテムを import する。

どれかが直ったら、その避け方を外せば、パターンがその形も測るようになる。3 と 4 を直したので、`generics` はまた、グループごとに
同じ型への trait `Measure` を 1 つずつ持つ（前はグループごとに名前を変えていた）。

出力の比較には限界がある。1 つのパターンのグループどうしは、いくつかの定数しか違わない。zustc が 2 つのグループのアイテムを
取り違えても（3 のように）、チェックサムは正しいままのことがある。上の再現は、グループの 1 つを手で変えて作った。`generics` は u32 の impl をグループごとに変えたので、別のグループの impl を
取ると出力が変わる。

## 確かめたこと

- すべてのパターンと `mix` を、N = 1、2、3、4 で、編集の有無の両方について: zustc の出力は rustc のものと同じ。
- `tests/bench_patterns.rs` と、`tests/driver2.rs` の 4 つの probe。
- 一時ディレクトリにプログラムをビルドするテスト（`tests/bench_patterns.rs`、`ast_codegen.rs`、`driver2.rs`、
  `panic_loc.rs`）は、利用者だけが入れる新しいディレクトリを作り、既にあるもの（共有の機械で他の利用者が作ったもの）は
  使わずに失敗する。
- `cargo test --release --workspace`、`bench/rust_ui.py --check`。
- KPI: pull request の `KPI` ワークフローのコメントに、ベースとの差がある。`patterns check` と `patterns edit` は
  新しいので、ベースには値がない。
