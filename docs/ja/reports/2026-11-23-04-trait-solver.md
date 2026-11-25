# プログラムのトレイトの trait solver: 先に Lean でモデル化・証明してから実装する

日付: 2026-11-23

zustc はトレイトのゴールを文字列の照合で決めていた。`Program::implements` はトレイトにブランケット impl が 1 つでもあれば
どの型にも「実装している」と答え (`impl<T> Foo for T where T: Bar` があるとどの型も `Foo`)、境界のトレイト引数は読まず、
impl の境界をたどるのは構造体のフィールドの well-formedness 検査 (`bound_holds`。「深さ 16 を超えたら成り立つ扱い」)
だけだった。この作業では、トレイトのゴールが成り立つとは何かを定め、それに対するアルゴリズムを Lean 4 で書いて証明し、
そのアルゴリズムを zustc に実装した。

- モデルと証明: [formal/lean/](../../../formal/lean/README.md) (`formal/run.sh lean`)
- 実装: [crates/sema/src/traits.rs](../../../crates/sema/src/traits.rs)

## モデル

型は木 (impl のパラメータの `var`、`con`、`app`) で、ゴール `T: Tr<A>` は木 `Tr T A`。なので impl は Horn 節になる:
頭 (self 型とトレイト引数) と本体 (パラメータの境界。where 節を含む)。仕様 `Holds` は impl と項目自身の仮定 (プレースホルダの
境界) で閉じた最小のゴールの集合。帰納的なので、`impl<T: B> A for T` と `impl<T: A> B for T` はどちらも証明しない (rustc: E0275)。

`solve` がアルゴリズム: 頭がゴールに合う (一方向のマッチング) impl を順に試し、その where 節を深さ優先・燃料を減らして証明し、
証明中のゴールをスタックに持つ。一周して戻ってきたゴールはその候補を失敗させる。答えは `yes`・`no`・`overflow`
(燃料切れ、または where 節が頭で束縛されないパラメータを名指していて部分ゴールが ground でない)。

| 定理 | zustc にとっての意味 |
|---|---|
| `solve_yes_sound` | `yes` は成り立つゴールにだけ |
| `solve_no_sound` | `no` は成り立たないゴールにだけ: `no` で出す E0277 は誤検出にならない |
| `solve_complete` | 燃料が足りれば成り立つゴールは `yes` (where 節が頭で束縛されるパラメータだけを名指す impl について) |
| `solve_no_sound_weaken` | zustc が読めない部分を impl から落としても `no` は健全のまま (下記) |
| `mayUnify_sound` / `mayUnify_complete`, `select_unique` | impl の頭の重なり (E0119): 線形な頭では正確。単一化しない頭どうしは競合しない |

注意の要るのはスタック。サイクル由来の `no` が健全なのは、証明はいつでも 1 本の道で同じゴールを 2 度証明しない形に切り詰め
られるから (`HA_cut`。証明の高さに関する強い帰納法)。証明中のゴールに戻るだけの候補は、どの証明も失わない。すべて core Lean
(Mathlib なし) で、`Axioms.lean` で定理が Lean の標準公理だけに依ること (`sorry` がないこと) を確かめる。`Examples.lean` は
下の tests/ui のケースをモデルで解く (`by decide`)。

## 弱める: zustc が読めないもの

zustc の要約は impl の言うことをすべては持っていない。実装は推測しない: 読めない where 節 (関連型への境界、具体型への境界) は
本体から落とす。解決できない self 型・トレイト引数は何にでも合う。頭の関連型は何にでも合う。ゴールの `i32` / `f64` は、impl が
型を選ぶ前に zustc が既定にしたリテラルかもしれない (`impl A<isize> for isize` での `f(0, 2)`) ので、どの整数 / 浮動小数点にも
合う。どれも zustc の節が本物の impl を *覆う* ようにするもので、`solve_no_sound_weaken` が、弱めたプログラムの `no` は本物の
`no` であることを示す。読まない境界で説明がつくかもしれないプレースホルダ・トレイトオブジェクトの `no` は `overflow` にする。

`overflow` は「分からない」で、zustc は受け入れる。

## 実装

`TraitDb::build` がプログラム自身のトレイトの impl を節にする (`Program::build_with` で 1 回)。`Cx::solve` はモデルの `solve`:
同じ `and` / `or` の表 (単体テストが 1 ケースずつ確かめる)、正確なマッチング (`&mut T` は `&T` でない、配列の長さが一致する。
推論用に寛容な `match_ty` は使わない)、燃料 48、1 問あたり 20,000 ゴールの予算。プログラムのものでないトレイトは
`Program::implements` に聞き、その「いいえ」はこれまで通りスカラーと普通の構造体と prelude のトレイトのときだけ信じる。

使う場所:

- `Insts::request` (ジェネリックな呼び出しすべての境界): `no` は E0277。ゴールがプレースホルダを含まなければ呼び出し側の
  型パラメータによらず成り立つので、どこからも呼ばれないジェネリック関数からも報告する (`traits::SOLVED_NO` の印。
  プレースホルダの回はこれを通す);
- `bound_holds` (static とフィールドの境界): `holds != No` になった;
- `wf_error`: impl ヘッダのトレイト引数 (その中の型と、トレイトがパラメータに付ける境界: `trait Getter<T: Clone2>` と
  `impl Getter<isize> for isize`) と、自由関数のシグネチャ (`struct Foo<T: Trait>` と `fn explode(x: Foo<u32>)`) に広げた。

## 結果

tests/ui (rust-lang/rust 1.100.0)、main との比較:

| 種類 | 前 | 後 |
|---|---:|---:|
| run-pass | 1832 | 1832 |
| check-pass | 1512 | 1512 |
| build-pass | 102 | 102 |
| compile-fail | 4497 | 4509 |

通っていたテストで落ちたものはない。新たに通る compile-fail の 12 件 (どれもテストが指す行で、テストの意図した理由で):
`traits/inductive-overflow/{simultaneous,supertrait,two-traits}.rs` (サイクル)、`traits/issue-18400.rs`、
`traits/next-solver/diagnostics/where-clause-doesnt-apply.rs`、`errors/trait-bound-error-spans/blame-trait-error.rs`、
`suggestions/issue-84973-2.rs`、`typeck/issue-90804-incorrect-reference-suggestion.rs` (ブランケット impl の where 節)、
`traits/impl-bounds-checking.rs`、`trait-bounds/super-assoc-mismatch.rs`、`traits/bound/on-structs-and-enums-in-impls.rs`
(impl ヘッダのトレイト引数)、`traits/bound/on-structs-and-enums-in-fns.rs` (関数のシグネチャ)。

途中で見つけて上の数字の前に直した退行が 2 つ: 境界の引数は呼び先自身のパラメータを名指す (`U: Eq<T>` の `T` を同名の構造体と
解決していた)、引数のないジェネリック構造体の基は ground な型ではない。

新しいテスト: `tests/compile_errors/trait_solver_*.rs`、`examples/trait_solver.rs` (期待出力は rustc のもの)、
`tests/check.rs::trait_solver_no_in_uncalled_generic_fn`、`traits::tests::and_or_tables`。

## 残っているもの

- コヒーレンスはモデルでは証明した (`mayUnify`) が、zustc の E0119 検査は古いまま。
- auto trait (余帰納的) はモデルにない。zustc は構造的な `auto_trait` のまま。
- 関連型: 関連型への境界 (`where <T as Mirror>::Mirror: Foo`) は落とす (弱める)。正規化はモデル化していない。
- impl の義務としてのスーパートレイト (`impl<T> Complete for T` は `T: Partial` が要る。traits/issue-43784-supertrait.rs) と、
  dyn への型強制での E0277 (`traits/coercion-generic-bad.rs`) はまだ検査しない。
- 引数の違う同じトレイトの impl どうしのメソッド選択 (`impl Conv<u64> for u8` と `impl Conv<bool> for u8`) が誤った方を選ぶ。
  これはメソッド探索の問題で solver ではない。
- `tests/selfhost.sh` は main ですでに失敗する (ビルドスクリプトが `regions` を含めていない)。修理されたときのために
  `traits.rs` を program / compiler のスクリプトに足しておいた。
