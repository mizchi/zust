# zustscript (試作)

zust を土台にした、フロントエンド向け言語の試作。目標は、表面は TS 風の高水準な言語で、必要な箇所だけ Rust と同じ低レベルで書け、境界(コールバックの寿命を含む)をコンパイラが面倒を見ること。

- `zsc x.zsc -o out/` は次を書き出す
  - `x.js`: 読める ES module。JS 層の本体と、wasm 層のグルー
  - `x.d.ts`
  - `x.wasm.rs` と `x.wasm`: `wasm fn` があるときだけ。zustc でコンパイルする
- **zust の crate には手を入れていない**
  - zust のワークスペース外の独立した crate(`[workspace]` を自前で持つ)
  - zustc は外部コマンドとして呼ぶだけ
  - 本体側の変更は `.gitignore` の 2 行のみ
- テスト: `cargo test --release`
  - node が必要
  - app の例は zust 側の `target/release/zustc` を使う
  - `.d.ts` の検査は tsc を使う(環境変数 `TSC` か PATH 上の `tsc`。なければ飛ばす)

## 文法(現時点の案と決定事項)

決定事項:

- クロージャは `(x) => e` に一本化する
- 数値は `i32` / `u32` / `i64` / `f64` で、暗黙変換はしない(`Int` / `Float` の別名は置かない)
- class は持たない
- effect 多相は入れず、簡単な制約で済ませる(将来の拡張の余地は残す)
- 配列は TS の `readonly T[]` そのもの。JS の配列をコピーも変換もなしに渡せるので、コードサイズとブリッジの両面で有利。不変として扱い、`push` は新しい配列を作る
- `.d.ts` の数値は branded type にする

```rust
// データ: レコード、enum (代数的データ型)。class はなく、振る舞いは関数と match で書く
type Todo = { id: i32, title: String, done: bool }
enum Filter { All, Active, Done }          // データなし → "All" | "Active" | "Done"
enum Option<T> { None, Some(T) }           // データあり → { TAG: "Some", _0: x } (TS の判別共用体)

// 関数: トップレベルのシグネチャは注釈必須、本体は推論。let は不変
pub fn visible(todos: Array<Todo>, f: Filter) -> Array<Todo> {
  todos |> .filter((t) => match f { All => true, Active => !t.done, Done => t.done })
}
fn toggle(t: Todo) -> Todo { Todo { done: !t.done, ..t } }   // 更新は新しい値を作る

// num-traits 相当: 数値の bound を持つ関数は型ごとに単相化される (sum$i32 / sum$f64 / ...)
pub fn sum<T: Num>(xs: Array<T>) -> T { xs.fold(T::zero(), (acc, x) => acc + x) }

// JSX: コンポーネントは `fn Name(props: Record) -> Jsx`。props は型検査される
pub fn Row(props: RowProps) -> Jsx { <li onClick={() => props.onPick(props.sample.label)}>{props.sample.label}</li> }

// JS の関数を手書きで宣言して取り込む (TS の型の自動取り込みはまだない)
extern "react" fn useState<T>(init: T) -> (T, fn(T) -> ()) = "useState";
```

### 数値(AssemblyScript と同じ程度の表現力)

| 型 | JS での表現 | 演算 |
|---|---|---|
| `i32` | number | `(a + b) \| 0`、`Math.imul`、`/` と `%` は 0 除算で例外を投げ、結果を切り捨てる |
| `u32` | number | `>>> 0` で wrap する |
| `i64` | BigInt | `BigInt.asIntN(64, ..)`。2^53 を超えても正確 |
| `f64` | number | そのまま |

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
  - pub なものは、bound が許すすべての型の分を export する
  - `.d.ts` も型ごとに出す

### パイプライン `|>`

- 左辺を第1引数に入れる: `x |> f(a)` は `f(x, a)`、`x |> f` は `f(x)`
- メソッドにも使える: `xs |> .map(g)` は `xs.map(g)`

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

- JS では、effect を持つ関数は `function*`、`perform` は `yield`、その呼び出しは `yield*` になる。`handle` は深いハンドラ `$handle`(30 行の実行時ヘルパ)で処理する
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
pub wasm fn stddev(xs: &[f64]) -> f64 { ... }                       // 本体は Rust。zustc がコンパイルする
pub wasm fn map_sum(xs: &[f64], f: &dyn Fn(f64) -> f64) -> f64 { ... }  // 借用コールバック
pub wasm fn call_twice(f: Box<dyn Fn(i32) -> i32>, x: i32) -> i32 { ... } // 所有コールバック
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
| A. Rust に変換できる高水準言語 | JS 層のコードを Rust ソースに落とし、zustc で wasm/native にする | 不変のレコードや enum は Rust に落とせる。ただしクロージャ、GC 前提の共有(`Rc` だらけになる)、effect(Rust にはジェネレータがない)を落とすのが難しい | **部分的に採る**: effect を持たず、数値と配列を中心にした一階の部分集合(まず `T: Num` の関数)だけを Rust に落とせば、同じソースを JS でも wasm でも実行できる。zust の型付き IR が効いてくるのはここ |
| B. 全部 wasm に変換する前提で効率化 | UI まで wasm に置き、メモリモデルを wasm 向けにする | DOM には JS 経由でしか触れないので、境界の量は C より増える。サイズは 1〜4 の改善をしても JS 1.8 KB(gzip)に対して桁が違う | **単独の方向にはしない**。1〜4 の改善は C の wasm の島にそのまま効くので、別の作業として進める価値はある |

結論:

- C を本線にする
- A は「JS 層の純粋な数値の部分集合を wasm にも落とせる」という形で C に合流させる
- B のメモリモデルの改善は zust 本体の wasm バックエンドの課題として切り出す

## 段階的な目安(見積もりの更新)

| 段階 | 内容 | 目安 | 備考 |
|---|---|---|---|
| 済 | 試作(この crate) | - | 型付き IR なしで成立 |
| 1 | 言語として固める: モジュールと import、無名のレコード型、`Option`/`Result` の標準化、文字列の補間、エラーの改善、TS からの型名の衝突回避 | 2〜3 週 | |
| 2 | 道具: 増分ビルド(アイテム単位のハッシュは zust の lexer の考え方を流用)、LSP(診断、hover、定義へ移動)、source map | 3〜5 週 | zust の `ide` / `analyzer` の作りを参考に別 crate で |
| 3 | effect 多相(関数型の effect 行)、async の effect(Promise で継続を再開) | 2〜4 週 | 生成はジェネレータのまま |
| 4 | 方向 A の合流: JS 層の数値部分集合を Rust に落とす(`T: Num` の関数から) | 3〜6 週 | ここで zust に型付き IR があると楽になる |
| 5 | wasm バックエンドの改善(reactor、stackifier、wasm32 レイアウト) | 1〜2 か月 | zust 本体の作業 |
| 6 | TS の型の取り込み(限定した部分集合、tsc API による抽出) | 範囲次第 | 今は手書きの extern |

- 小さな実アプリ(React 上)が書けるまで: 段階 1〜2 で 1.5〜2 か月
- wasm の島が実用サイズになるまで: 段階 5 が必要

## 未決事項

- 配列の更新が多い箇所の計算量: `readonly T[]` に決めたので、`push` は O(n) のコピーになる。ループのように積み上げる用途には、`fold` で一度に作る、あるいは wasm 層に寄せる方針で足りるかを確かめる
- 文字列の長さ(`len` は今 UTF-16 単位。Rust の byte 単位とずれる)
- brand のコンストラクタ名(`i32` など)が、利用者の関数名と衝突しうる。今は検査していない
- wasm 層から JS 層の関数(コールバックでない普通の関数)を呼ぶ方向
- wasm 側で panic したときに所有コールバックが漏れること(今は abort して例外になり、表に残る)
- 試作の割り切り
  - リテラルパターンの i64 比較は `==` で済ませている
  - 引数の位置に置いた `match` は即時関数になる(評価順は保たれる)
