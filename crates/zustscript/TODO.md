# zustscript の TODO と、あとで決めること

現在の仕様は [README.md](README.md)、ML 用途と vector IR の計画は [docs/plan.md](docs/plan.md) にある。

## Rust 出力とガイドの次の作業

通常の関数の `--emit-rust`、`@wasm` からの通常関数呼び出し、JS 専用出力での `@wasm` の診断は [#119](https://github.com/mizchi/zust/pull/119) で実装済み。以下はそのマージ後の `main`（`61293a7f`）を基準とした残件。

### 先に直すこと・決めること

- [ ] **Rust 2024 の予約語を含むコードを正しく出力する。** `type Reading = { gen: i32 }` と `export fn read(x: Reading) -> i32 { x.gen }` は `--emit-rust` に成功するが、生成した宣言とフィールド参照の `gen` を `rustc --edition=2024` が拒否する。名前変換を両方に適用し、`gen` と `gen_` が同居するレコードを別の Rust モジュールから読み書きする回帰テストを追加する。
- [ ] **手書きの Rust を含むときの `--all-js` の契約を決める。** `@wasm fn` はエラーになる一方、`export wasm fn twice(x: i32) -> i32 { x + x }` を CLI に渡すと `zustc` が起動して `.wasm` ができる。ブラウザ版は同じ手書き関数に警告とスタブを出す。JS だけを要求した場合にビルドを拒否するか、現行の混在出力を許すかを決め、両方の入口で診断と生成物をテストする。
- [ ] **既存レビューの正しさに関わる指摘を再判定する。** [#35](https://github.com/mizchi/zust/issues/35) は未完了で、記録の一部は過去のコミットを基準にしている。所有権・名前衝突・SIMD の各指摘を現行テストと突き合わせ、修正済みの項目には対応コミット、未解決の項目には回帰テストを対応付ける。[元の調査記録](docs/review-31.md)

### 回帰テストと初見評価

- [ ] **記事の生成コード例を CI で照合する。** 入門・言語別ガイド・変換コスト記事と PC 用サイドバーは公開済み。現在の Pages ビルドは Markdown を HTML にし、ハイライト前後のコードが一致することを検査する。Rust 向け記事の `peak` と TypeScript 向け記事の `stock_change` を本文からコンパイルし、掲載した抜粋を実出力と照合する。通常関数の Rust 版も `rustc` で実行し、空配列・負数・入出庫イベントの結果を確かめる。この検査をコンパイラ変更時にも走らせる。
- [ ] **公開関数の初見評価を本流に取り込み、union の名前付きパターンを評価する。** [evals/publish-functions/](evals/publish-functions/README.md) に課題・採点・再実行手順がある。幅と高さがともに `number` のとき、TS の宣言順を入れ替えると位置パターンの意味が変わる。既存の評価記録を保存し、提案 001 の名前付きパターンでも同じ課題を試す。縦長・横長のどちらでも、宣言順にかかわらず幅 ÷ 高さを返せるかで比較する。

### 計測してから決める最適化

- [ ] **Wasm 内の中間配列と、JS 境界の転送を別々に測る。** `evens_squared` は `filter` と `map` の間に `Vec` を作る。同じ入出力の一回走査版と比較する。`relu(dense(...))` では前段の出力が JS を往復するので、呼び出し回数と転送バイト数を測り、Wasm 側に配列を保持するハンドルが必要か判断する。[生成コードと比較例](https://github.com/mizchi/zust/blob/main/crates/zustscript/docs/rust-costs.md)

## ターゲットと JS 相互運用

- [ ] [ターゲット属性とビルドの設計方針](docs/target-spec.md)を実装する。`@simd` の Wasm 固定、`@js` の DOM ラッパー、`@rust` の Rust モジュール取り込みを、同一ソースの web・JS 単独・Rust ビルドで検証する。
- [ ] 手書き `wasm fn` の `&[u8]` / `Vec<u8>` を JS の `Uint8Array` につなぐ。部分ビューと所有権を生成ラッパー経由で検証し、未使用時の増分 0 B と、使用時の JS・Wasm サイズを測る。[JS ヘルパー試作と基準値](docs/target-spec.md#配布サイズと採用範囲)

## 文法の提案の状態

提案は [evals/syntax/](evals/syntax/README.md) の改善ループ(提案 → ルーブリックの評価 → 改訂)で進めている。**どれもまだ実装していない**(今の zsc の文法は README のとおり)。

| 提案 | 状態 | 平均点の推移 |
|---|---|---|
| [001 バリアントの修飾と名前付きフィールド](evals/syntax/proposals/001-variants.md) | r5(評価前) | 3.67 → 3.54 → 3.71 → 3.79 |
| [002 構造的なレコード型と TS 風の関数型](evals/syntax/proposals/002-anonymous-records.md) | r5(評価前) | 3.08 → 3.54 → 3.83 → 3.79 |
| [003 Option / Result、`?`、任意のフィールド](evals/syntax/proposals/003-option-result.md) | r2(評価前) | 3.38 |

決めたことは、各提案の `results/<提案>/summary.md` の「決定(ユーザー)」の節にある。

### 次にやること

1. 001 r5、002 r5、003 r2 を評価する。high がなくなったら、書かせる評価(`prompts/write.md`)に進む
2. 004 を書く: or パターン(`A | B =>`)、match のガード(`if` つきの腕)、バリアントを関数の値として渡す(`xs.map(Some)`)
3. 実装の順序を決める。003 は 001(候補つきの型変数)と 002(`Ty::Rec`)が前提
   - 001: パターンの Id と side table、候補つきの型変数、絞り込みのメタデータ、パターン行列の網羅性
   - 002: `Ty::Rec`、`{` の 2 トークンの判定、struct 名を形から作る lower
   - 003: prelude、`?` の脱糖(型検査の後の AST の書き換え)、境界での `undefined` の変換

## 実用に向けて: 足りない機能の棚卸し(2026-11)

zsc に小さなコードを通して確かめた(機能ごとに 1 つの `.zsc` を `--all-js` でコンパイル)。

**結論: 1 ファイルで完結する小さな React の画面や数値カーネルは書けるが、実アプリにはまだ足りない。**
足りないのは主に、`Option` / `Result`、JS の値の型、標準ライブラリの量。

今あるもの:

- レコード、enum、`match`、bound つきの総称関数・総称のレコード、クロージャ、`|>`、JSX
- 数値の型(i32 / u32 / i64 / f32 / f64 と typed array)
- 代数的エフェクト
- `@wasm` / `@simd`、`--emit-rust`
- `import type` での TS の型の読み込み、`extern fn`
- 文字列の補間
- ブラウザのプレイグラウンド(editor/)

### P0: 実アプリを書く前に要るもの

1. **済: `.zsc` どうしのモジュール**(README の「モジュール」)。`import { Shape, area } from "./shapes.zsc";` で型、`export fn`、コンポーネント、`@wasm export fn` を取り込む
   - 残り
     - 数値の bound を持つ総称関数、effect を perform する関数、Rust の `wasm fn` の取り込み
     - `@wasm fn` から取り込んだ関数を呼ぶこと(相手の Rust も一緒にコンパイルする)
     - `--emit-rust` でのモジュール(Rust の `mod` / `use` にする)
     - パッケージからの取り込み、プレイグラウンドでの複数ファイル
     - 名前を変えて取り込む(`import { area as shapeArea }`)、`export` の付かない型を取り込めないようにするか
2. **済: 非同期**(README の「非同期」)。`async fn`、`await`、`async (..) =>`、`Promise<T>`(JS の async / await のまま。`.d.ts` は `Promise<T>`)
   - 残り
     - effect を perform する async fn(段階 3 の「async の effect」: Promise で継続を再開する)
     - `Promise.all` などの組み合わせ(reject は 7 の `Promise<Result<T, E>>` で受けられる)
     - async の React コンポーネント(Server Components)は対象外
3. **済: `Option` / `Result` と `?`**(README の「Option / Result と `?`」。提案 003 の A / B / D)。prelude、`?`、`if let`、メソッドと `try_map`
   - 残り
     - 任意のフィールド `x?: T` と、TS との境界での `undefined` の変換(提案 003 の C)。`import type` で TS の任意のフィールドを読むのも、これが前提
     - `zsc-prelude.d.ts`(今は `.d.ts` ごとに `Option` / `Result` を宣言するので、TS 側で 2 つのモジュールを `export *` すると衝突する)
     - `@wasm` での `Option` / `Result` のメソッド(今は Rust の enum `ZsOption`。std の `Option` に下ろすか)
     - `unwrap()` / `expect()`、`while let`、JSX の子の `Option<Jsx>`(提案 003 の E)
     - ユーザーの enum の `None` と prelude の `None` を同じファイルで使うこと(001 の修飾と一緒に)
4. **済: JS の値の型**(README の「JS の値」)。`extern type`、そのプロパティ・メソッド・代入、`new X` のコンストラクタ、`null` を `None` に
   - 残り
     - `==`(同じ値か)、型引数を持つメソッド、静的メソッド(今は `extern fn` で `Math.max` などと書く)
     - TS の `.d.ts`(`lib.dom.d.ts` など)からの自動の取り込み(`import type` を関数とクラスに広げる)
     - `@wasm` での extern type(JS の値を handle として渡す)
     - 上の「ターゲットと JS 相互運用」の `@js` の DOM ラッパー([docs/target-spec.md](docs/target-spec.md))との整理
5. **済(一部): 標準ライブラリ**(README の「標準ライブラリ」)。配列・文字列・数値のメソッドを JS と `@wasm`(と `--emit-rust`)で同じ意味に
   - 残り
     - 済: `Map` / `Set`、JSON(`json_decode` / `json_encode`)、`Eq` / `Ord` / `Hash` の bound(README の各節)
     - `Map` / `Set` の残り: `@wasm` での `Map::from` / `entries()`(タプル)と境界、`union` などの集合演算、`for` で直接回すこと
     - JSON の残り: `@wasm`、i64(文字列で持つか)、動的な値の型(`Json` の enum)、フィールド名の付け替え
     - 乱数(JS 層だけ)、`enumerate` 相当(`for (i, x) in ..` と一緒に)、数値の定数(`f64::PI` など)
     - 型つき配列(`F64Array` など)の新しいメソッド
     - zustc の制限への回避: [#207](https://github.com/mizchi/zust/issues/207)(`Wire::get` が `Vec::get` を隠す: helper の名前を `decode` にした)、[#208](https://github.com/mizchi/zust/issues/208)(`vec![..]` の中のクロージャ: `includes` / `index_of` はクロージャなしの補助関数)、[#209](https://github.com/mizchi/zust/issues/209)(`parse::<f64>` が `strtod` を要る: `to_f64` はグルーの JS で読む)、[#214](https://github.com/mizchi/zust/issues/214)(`match` の結果を `[..]` で切ると型を見失う: 型を付けて束縛する)
6. **パターン**: or パターン(`A | B =>`)とガード(`n if n > 1 =>`)(提案 004)
7. **済: 例外**(README の「JS の値」)。`extern` の結果を `Result<T, String>`(メッセージ)か `Result<T, E>`(E は extern type: 投げた値)にすると、throw を `Err` で受ける。`Promise<Result<T, E>>` は reject も
   - 残り: zustscript の側で投げる手段(`panic` 相当)、境界の `pub fn` の `Result` を TS の例外にするか(下の 003 の未決事項)
8. **済: `return` と `loop`**(README の「ローカルな可変性とループ」)。JS では式の中の `return` を `?` と同じく文として前に書き出す。Rust でも `return` / `loop`
   - 残り: `break value`、ラベルつきの `break`、`handle` の中の `return`、`@wasm` のクロージャの中の `return`
9. **プログラムを書いて見つかったもの**(電卓、Todo アプリ、CSV の集計、ライフゲーム、粒子のシミュレーション、3 モジュールの非同期アプリ)。上の 5 / 6 以外で
   - 型: 型の別名(`type Env = Array<(String, f64)>`)、`impl` のメソッド、enum のレコード形のバリアント(`Eq` / `Ord` の bound は済)
   - パターン: `let P { x, y } = p`、クロージャの引数の `((a, b)) =>`、`for (i, x) in ..`、`while let`。網羅性で漏れたときのメッセージ(タプルや入れ子では「`_` を足す」としか言わない)
   - 文法: トップレベルの `const`、入れ子の `fn` と自分を呼ぶクロージャ、ビット演算子(`& | ^ << >>`)、`unwrap` / `panic`
   - JSX: 子の `Option<Jsx>`、`{...props}`、`cond && <x />`
   - `@wasm`: タプル、`Option` / `Result` の関数を取るメソッド(`map` など。`unwrap_or` `is_some` `ok` `ok_or` は済)
   - ツール: `zsc` が取り込む `.zsc` を一緒にコンパイルしない(ファイルごとに実行する)。`@wasm` の出力は `--inline-runtime` でも `zustscript-helper` パッケージを要する
   - 境界: DOM のイベントの型(`extern type` を手で書く)。JSON を型どおりに読むのは済(`json_decode`)

### P1: 使い心地

- メソッド(`impl`)とインターフェース(`trait`)を入れるかどうか。今は関数と `|>` だけ
- トップレベルの定数(`const N: i32 = 3;`)、関数の中の関数
- 可変のローカル変数をクロージャが捕まえられないこと(`let mut s = 0; xs.map((x) => { s += x; .. })` は型エラー)。イベントハンドラでは React の state を使うので困りにくいが、ガイドに書く
- 道具
  - LSP(診断、hover、定義へ移動)、フォーマッタ、source map(段階 2)
  - テストの書き方(`#[test]` 相当か、TS のテストから呼ぶか)
- プレイグラウンド: 複数ファイルと、`import type` で読む TS ファイルのタブ

### P2: 残りの細部

- テストの出力ディレクトリ: `tests/examples.rs` の `out_dir(name)` は `target/test-out/<name>` を使うので、同じチェックアウトで `cargo test` を 2 つ同時に走らせるとぶつかる。失敗を調べられるように出力は残したまま、プロセスごとの接尾辞などでヘルパーの側で分ける(mizchi/zust#146 のレビューで合意)

- `@wasm` の中の浮動小数点数の `to_string` と補間の穴: JS の `Number#toString`(往復できる最短の桁)を、`core::fmt` を使わずに書く必要がある(`format!` は wasm を大きくする。README の「文字列と `==`」の表)。最短の桁を求める Ryu などの表も、サイズを測って決める
- typed array をバイト列の形式に載せる
- zustscript-helper を npm に公開する
- enum の JS での表現を選べるようにする(`@tag`。下の「あとで決めること」)

## あとで決めること

### enum の JS での表現(`@tag`)

多くの未決事項がここに集まっている。001 / 003 はこの決定に依存しないように書いてある。

- 判別キー(今は `TAG`)と、データなしのバリアントの形(文字列かオブジェクトか)。`@tag(kind)` のように enum ごとに選べるか
- 別の enum の同名のバリアントが同じ TAG になること(`Parsed.None` と `Hit.None`)
- データなしの enum にデータつきのバリアントを足すと `.d.ts` の形が変わること
- TS 側でバリアントの値を作るコンストラクタ(`Shape.Circle(…)`、`Option` / `Result`)
- 公開関数の境界の `Option<T>` を `T | undefined` にするか、`Result` を JS の例外にするか(`@throws` のような印。wasm_bindgen と同じ)

### 001 バリアント

- 全バリアントに共通するフィールド `s.at` を、絞り込まずに読めるようにするか
- 絞り込みを `if` の条件にも広げるか(`if s is Circle { … }`、003 の `if let` と一緒に)
- `Shape::Circle` を型引数の中、フィールドの型、クロージャの引数、関数型の中に書けるようにするか
- 名前なしの `{ ...s, center }` を、絞り込まれた `s` の更新として読むか
- 同名の関数があるとき、位置引数のパターン `Pair(a, b)` と裸の `Empty` のパターンでも修飾を省くか
- 型の別名とバリアントの同名を、関数と同じ規則(重なる形だけ修飾)に緩めるか
- 残りに名前を付ける `Circle { r, ...rest }` と、`rest` の型
- 別のモジュールの enum のバリアントの解決(import と一緒に)

### 002 レコード

- 公開関数の無名の戻り値の型に、`.d.ts` で名前を付けるか
- `void` を `()` の別名として受け付けるか(型の位置だけ)
- 総称的な無名のレコード `{ value: T }`。総称的な別名に Rust の総称の別名を出すか
- spread を 2 つ以上書けるようにするか(`{ ...defaults, ...props }`)
- **決定: レコードと enum の値の `==` は中身で比べる**(JS は `$eq`、`@wasm` は `derive(PartialEq)`。README の「文字列と `==`」)
- TS から来た余分なプロパティ: JS 層では残り、`@wasm` と境界で作り直す型では落ちる

### 003 Option / Result

- `?.` と `??` の意味(今は予約。`?.` は Rust の意味 / TS の意味 / 予約のまま、`??` は `unwrap_or_else` の略記か)
- **決定: `if let Some(x) = o { .. } else { .. }`**(済。`match` に読み替える)。`while let` と、`if o is Some(x)` の形は未決
- `unwrap()` / `expect()`、`or` / `or_else` / `filter` / `flatten`
- `handle` の本体と arm の中の `?` を、どの関数から返すことにするか
- 関数の任意の引数(`fn f(x?: i32)`)と、バリアントの任意のフィールド
- 境界で包んだ関数が、渡るたびに新しい関数になること(React の `memo`)
- 例外を投げる `extern` の関数を `-> Result<T, String>` で受けること

### 文法の refine の残り(提案になっていないもの)

- **済: 文字列の補間 `` `hi ${name}` ``**(README の「文字列と `==`」)。`@wasm` では `format!` を使わず `push_str` と `zs_push_int` で組み立てる。残り: `@wasm` の中の浮動小数点数の穴(下の P2)
- モジュールと import(`import { a } from "./b.zsc"`、npm の型との接続)
  - **済: TS のファイルのプレーンなレコード型を `import type` で読む**(README の「TS の型の取り込み」)。残り: 任意のフィールド(003 の `Option` と一緒に)、総称型、`extends`、union(**済: 文字列リテラルの union と判別キーつきの union を enum として読む**。**済: その enum を `@wasm` との境界で渡す**。残り: zustscript の enum の表現を `@tag` で選べるようにする)、パッケージの .d.ts からの読み込み、関数の型(`extern` の代わり)
  - **済: 文字列や配列を含むレコードを `@wasm` との境界で渡す**: 補助のパッケージ zustscript-helper(TS)と crate zustscript_helper(Rust)の、1 つの固定したバイト列の形式で(README の「バイト列で越える値」)。**済: enum もこの形式に載せる**。残り: typed array をこの形式に載せる、zustscript-helper を npm に公開する(今は helper/ts をそのまま置く)
- ML 用途の文法: [docs/plan.md](docs/plan.md) の「TODO: ML 用途」

## 提案の中の、分かっている食い違い

r5 / r2 を書いた agent が報告したもの。次の改訂で直す。

- 001 の例の `Shape` には `Label { text: String }` があるので、002 の「`@wasm` で使えるのは Copy にできる形だけ」に照らすと `@wasm` で使えない(wasm の例は「Copy の形だとして」と断り書きにしてある)
- 001 の決定 9(公開関数の入口で TAG を検査し、内部の呼び出しは検査しない): `@wasm` の公開関数では、JS 層の中からの呼び出しも glue の検査を通る
- 001 の引数のあるバリアント(`if c { Parsed::Ok(1) } else { Ok(2) }`)は、`EK::If` が他の分岐の型を期待する型として渡すので、分岐の順で結果が変わりうる(003 は「書いた型」だけを数える規則で避けた。001 にも同じ規則を入れる)

## JS ターゲット: サイズ、速さ、TS との型の接続

計測は `node bench/js/kpi.mjs`(`--esbuild PATH`、`--save FILE`、`--against FILE`)。JS 比較用にサンプルの `@wasm` 属性を外し、`bench/js/workloads.zsc` とともに `--all-js` でコンパイルし、esbuild で bundle + minify + gzip したサイズと、`w_*` の速さを出す。ベースラインは `bench/js/baseline.json`(node 26.13.1、esbuild 0.28.2)。

決めたこと(順に入れて、1 つずつ効果を測る):

1. **済: effect の tail-resumptive な handler を、直接の関数呼び出しにした**(`check::direct_effects`、`$perform` / `$handleD`)
   - handler のすべての arm が末尾で `k(x)` を呼ぶだけで、公開関数から外へ出ず、generator の effect と混ざらない effect は、普通の呼び出しになる。それ以外は generator のまま
   - `w_effects` は 676 ms → 6.5 ms(約 100 倍)。結果は同じ。operation を 100 万回しても、スタックはあふれない(回帰テスト `many_ops`)
   - **済: generator の deep handler のスタックのあふれ**: 命令を 1 つのループ(`$run`)で実行し、フレームをヒープ上のスタックに積む(trampoline)。1 つの `handle` の中で operation を 10 万回しても、あふれない。速さも 4〜7 倍
2. **済: 実行時の補助関数を共通の runtime モジュールにした**: `zsc-runtime.js` / `.d.ts`(補助関数と、数値の brand とそのコンストラクタ)を出力先に 1 つ書き、各モジュールは使うものだけ import する。モジュールは使う brand を re-export するので、TS 側の import は変わらない。`--inline-runtime` で、これまでどおりモジュールの中に書く
   - 1 つのモジュールの bundle は変わらない(tree-shaking)。全モジュールを 1 つに bundle すると minify 後で 18,376 → 14,249 バイト(−22%)、gzip 後で −6.5%。zsc が出す .js は −30%
   - 003 の prelude(`Option` / `Result` の型)も、ここに置く予定
3. **済: TS との境界の検査に tsrs を使う**(`zsc --check-externs`、`npm i tsrs`。tsc と同じ診断で、今の .d.ts の検査は 0.89 秒 → 0.06 秒)
   - zsc は `<stem>.externs.ts` を書く。各 `extern` の実体を、zustscript の型(数値は `number`)の変数に代入する: `const $c0: <T>(a0: T) => readonly [T, (a0: T) => void] = useState;`。引数は反変、戻り値は共変で確かめられる。TS の型検査を自前で実装しない
   - tsrs のエラーは、その `extern` の位置に戻して報告する(`the TS declaration of \`useId\` does not fit this signature: Type '() => string' is not assignable to type '() => number'`)
   - 実際の `@types/react`(React 19)で、`useState` は合い、戻り値を誤った `useId` は検出された。CI は tsrs も入れて、テスト `externs_match_ts` を走らせる
   - tsrs は Rust 1.100 が要り(この環境は 1.97)、Rust ライブラリとしての安定した API もないので、zustc と同じく外部コマンドとして使う
   - 次: npm の .d.ts から `extern` の宣言を作る(`zsc import-types`)。tsrs の Node API の型の問い合わせはまだ partial

まだ決めていないこと:

- 式の中の即時関数(`iife`)をなくす変換(003 の `?` の脱糖と同じ AST の書き換えに載せる)
- enum の表現(`@tag`)がサイズに効く分(`TAG: "Circle"` と `_0` のキー)
- `|> .map(..) |> .filter(..)` の連鎖を 1 つのループにまとめるか

## `@wasm` の所有権の推論の残り

所有権の推論(README の「所有権の推論」)は入った。残り:

- **済: JS 層の push もその場でする**(`own::inplace_pushes`、README の「所有権の推論」)。`ramp_sum(100000)` は 50.9 秒 → 8.2 ms
  - 残り: `fold` の累積値(`xs.fold([], (acc, x) => acc.push(x))`)は、まだコピーする(初期値が新しい配列で、closure の結果もそうなら、その場にできる)。`let mut` から `let mut` へ移すと、移した先は対象から外れる
- **済: 所有するローカル変数への長い借用**: `let firsts = first_pts(ps)` は、`ps` が所有するローカル変数でもコピーしない。ブロックの残りが `ps` を代入しないなら借用のままにして、ブロックの終わりまで `ps` を凍結する(move せず clone)。ブロックやアームの外へ出る借用、結果として返る借用は所有にする
- lowering の回数: 今は各関数を平均 3 回 lowering する(鎖の例)。callee から先に lowering する順にすれば、ほぼ 2 回になる
- **済: 文字列(`String` / `&str`)と、配列・レコード・enum の `==`**(README の「文字列と `==`」)。文字列・配列を含むレコードの境界転送も [wire.zsc](tests/wire.zsc) で検証済み。残り: タプル、文字列のパターン(`match s { "a" => .. }`)、浮動小数点数の `to_string`(JS の書式を Rust で再現する)
- (推論とは別の問題) 検査器が、関数が多いと線形より遅くなる(1,600 関数で関数あたりの費用が増える)。`check_file` のソートが目立つ

## 実装の残り(文法以外)

- vector IR: [mizchi/zust#30](https://github.com/mizchi/zust/issues/30)(別のセッションで進行中)
