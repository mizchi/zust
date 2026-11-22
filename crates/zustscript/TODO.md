# zustscript の TODO と、あとで決めること

README.md は「今あるもの」、[docs/plan.md](docs/plan.md) は計画のメモ(ML 用途、vector IR との関係)。このファイルは、文法の改善ループで**決めずに残したこと**と、次にやることの一覧。

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

## あとで決めること

### enum の JS での表現(`@tag`)

多くの未決事項がここに集まっている。001 / 003 はこの決定に依存しないように書いてある。

- 判別キー(今は `TAG`)と、データなしのバリアントの形(文字列かオブジェクトか)。`@tag(kind)` のように enum ごとに選べるか
- 別の enum の同名のバリアントが同じ TAG になること(`Parsed.None` と `Hit.None`)
- データなしの enum にデータつきのバリアントを足すと `.d.ts` の形が変わること
- TS 側でバリアントの値を作るコンストラクタ(`Shape.Circle(…)`、`Option` / `Result`)
- `pub` な関数の境界の `Option<T>` を `T | undefined` にするか、`Result` を JS の例外にするか(`@throws` のような印。wasm_bindgen と同じ)

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

- `pub` な関数の無名の戻り値の型に、`.d.ts` で名前を付けるか
- `void` を `()` の別名として受け付けるか(型の位置だけ)
- 総称的な無名のレコード `{ value: T }`。総称的な別名に Rust の総称の別名を出すか
- spread を 2 つ以上書けるようにするか(`{ ...defaults, ...props }`)
- レコードと enum の値の `==`(構造的な等しさ。`@wasm` の `derive(PartialEq)` と結果を揃える)
- TS から来た余分なプロパティ: JS 層では残り、`@wasm` と境界で作り直す型では落ちる

### 003 Option / Result

- `?.` と `??` の意味(今は予約。`?.` は Rust の意味 / TS の意味 / 予約のまま、`??` は `unwrap_or_else` の略記か)
- `if let Some(x) = o`(または `if o is Some(x)`)
- `unwrap()` / `expect()`、`or` / `or_else` / `filter` / `flatten`
- `handle` の本体と arm の中の `?` を、どの関数から返すことにするか
- 関数の任意の引数(`fn f(x?: i32)`)と、バリアントの任意のフィールド
- 境界で包んだ関数が、渡るたびに新しい関数になること(React の `memo`)
- 例外を投げる `extern` の関数を `-> Result<T, String>` で受けること

### 文法の refine の残り(提案になっていないもの)

- 文字列の補間 `` `hi ${name}` ``
- モジュールと import(`import { a } from "./b.zsc"`、npm の型との接続)
- ML 用途の文法: [docs/plan.md](docs/plan.md) の「TODO: ML 用途」

## 提案の中の、分かっている食い違い

r5 / r2 を書いた agent が報告したもの。次の改訂で直す。

- 001 の例の `Shape` には `Label { text: String }` があるので、002 の「`@wasm` で使えるのは Copy にできる形だけ」に照らすと `@wasm` で使えない(wasm の例は「Copy の形だとして」と断り書きにしてある)
- 001 の決定 9(`pub` の入口で TAG を検査し、内部の呼び出しは検査しない): `@wasm` の `pub` な関数では、JS 層の中からの呼び出しも glue の検査を通る
- 001 の引数のあるバリアント(`if c { Parsed::Ok(1) } else { Ok(2) }`)は、`EK::If` が他の分岐の型を期待する型として渡すので、分岐の順で結果が変わりうる(003 は「書いた型」だけを数える規則で避けた。001 にも同じ規則を入れる)

## JS ターゲット: サイズ、速さ、TS との型の接続

計測は `node bench/js/kpi.mjs`(`--esbuild PATH`、`--save FILE`、`--against FILE`)。`examples/*.zsc` と `bench/js/workloads.zsc` を `--all-js` でコンパイルし、esbuild で bundle + minify + gzip したサイズと、`w_*` の速さを出す。ベースラインは `bench/js/baseline.json`(node 26.13.1、esbuild 0.28.2)。

決めたこと(順に入れて、1 つずつ効果を測る):

1. **済: effect の tail-resumptive な handler を、直接の関数呼び出しにした**(`check::direct_effects`、`$perform` / `$handleD`)
   - handler のすべての arm が末尾で `k(x)` を呼ぶだけで、`pub` な関数から外へ出ず、generator の effect と混ざらない effect は、普通の呼び出しになる。それ以外は generator のまま
   - `w_effects` は 676 ms → 6.5 ms(約 100 倍)。結果は同じ。operation を 100 万回しても、スタックはあふれない(回帰テスト `many_ops`)
   - **残る不具合**: generator の deep handler は、operation のたびに JS のスタックが深くなり、1 つの `handle` の中で operation が 1000 回前後になるとあふれる。ループの形の再開(trampoline)が要る
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
- 文字列(`String` / `&str`)、タプル、配列の `==`(構造的な等しさ。002 の「レコードと enum の値の `==`」と一緒に決める)
- 配列を含むレコードを、JS との境界で渡す(今は境界ではスカラーだけのレコード)
- (推論とは別の問題) 検査器が、関数が多いと線形より遅くなる(1,600 関数で関数あたりの費用が増える)。`check_file` のソートが目立つ

## 実装の残り(文法以外)

- PR #31 のレビューで直さずに残した指摘: [mizchi/zust#35](https://github.com/mizchi/zust/issues/35)(検証の結果は [docs/review-31.md](docs/review-31.md))
- vector IR: [mizchi/zust#30](https://github.com/mizchi/zust/issues/30)(別のセッションで進行中)
