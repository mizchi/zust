# 調査: Rust コンパイラの最前線（2026 年 11 月）と、zustc がそこから取り入れるもの

日付: 2026-11-23

Rust のコンパイラ、言語、ツールで上流が新しく何をしているかと、その項目ごとに zustc がどこにいるかを記録する。ロードマップが、過去の rustc ではなく最前線を追うようにするため。出典は 2026 年のプロジェクト目標、1.95〜1.100 のリリースノート、Rust のブログ、tracking issue（リンクは末尾）。zustc の状態は 2026-11-23 に 4 コアで、次のもので測った。

- rustc 1.97.0 stable（ここに入っているもの）
- rustc 1.102.0-nightly（2026-11-22）。Polonius Alpha が既定で有効
- zustc main d2146f7（`cargo build --release`）
- rustc の tests/ui 1.100.0（`python3 bench/rust_ui.py --fetch`）。このリポジトリが開発に使っている版

3 つの項目は issue に書いた: Polonius Alpha（[#1](https://github.com/mizchi/zust/issues/1#issuecomment-7110120344)）、次世代 trait solver（[#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011)）、並列フロントエンド（[#101](https://github.com/mizchi/zust/issues/101)）。

## 1. まとめ

| 分野 | 上流（2026-11） | zustc | すること |
|---|---|---|---|
| borrow check | Polonius Alpha が 2026-08-04 から nightly で既定。2026 年末までに stable の予定 | もともと Polonius 寄り（ローカルごとに origin 1 つ）。`nll/polonius` 34 件のうち 5 件で Alpha と食い違う（ほかに未対応が 2 件） | #1: `Bk::forever` の代わりに位置に依存する生存解析 |
| trait の解決 | 新 solver が coherence（1.84）と orphan check（1.100）で使われている。`-Znext-solver=globally` の安定化が 2026 年の目標 | `implements(t, tr) -> bool` で、名前で答える。`traits/next-solver` 193 件で誤った拒否 26 件、見逃し 23 件 | #2: 確からしさを持つ goal、入れ子の where 句、goal としての正規化 |
| 並列フロントエンド | 推進中。2026-11-16 から nightly で既定になったという報道がある（未確認） | `check` は 1 プロセスで動く。`-j 4` の cold build と同じくらい遅い | #101: check の wave ごとに fork |
| 再ビルド | Relink Don't Rebuild: 最初の実装が open な PR にある | 関数の単位ではできている（callee の本体が変わっても caller のコードはそのまま） | クレートをまたいで同じことをする（依存の解析結果を使い回す） |
| デバッグ用のコード生成 | TPDE（シングルパスのバックエンド、LLVM -O0 の 10〜20 倍速い）を実験的な選択肢にする | シングルパスのレジスタ割り当ての Cranelift と、minclif | minclif を TPDE の設計と比べる |
| ジェネリクス | 多相なコード生成の実験（型引数ごとの隠れた辞書、レイアウトによる共有） | 型引数ごとにインスタンス 1 つ | ロードマップの「レイアウトをキーにインスタンスを共有」が同じ考え |
| 言語 | `!` が stable、独自のアロケータ、`cfg_select!`、`if let` ガード、新しい範囲型、多くの API | 試した 12 件のうち 4 件が通る | 残り 8 件を埋める（4 節） |

## 2. ビルドの速さ

2026 年のロードマップ ["Fast Builds"](https://goals.rust-lang.org/2026/roadmap-fast-builds.html) は、バッチのコンパイルについて「簡単に取れる改善はもうない」と書いている。作業は 12 ある。zustc に関わるもの:

- **Relink Don't Rebuild（RDR）**、tracking issue [rust-lang/rust#158844](https://github.com/rust-lang/rust/issues/158844)。クレートのメタデータに公開 API のハッシュを持たせ、それが変わったときだけ依存側を再ビルドする。変わらなければリンクだけをやり直す。よくある変更で再ビルドを 5〜10 倍速くするのが目標。最初の実装は rust-lang/rust#156940（open）にある。残っている問題:
  - スパンがハッシュに入るので、本体や doc コメントを直しても依存側は再ビルドされる
  - v0 マングリング（1.97 から既定）と両立しない
  - インクリメンタルでないビルドで約 3%、何も変えないインクリメンタルビルドで約 25% のコストがかかる

  zustc はクレートの中ではこれをすでにしていて、スパンの問題もない。関数のキーには callee のシグネチャしか入らず、ハッシュは正規化したトークンのもの（`docs/architecture.md` の「インクリメンタルの要点」）。クレートをまたぐとまだで、依存する側のクレートは依存先のソースを解析し直している。これはロードマップの「依存クレートの解析結果を依存する側で使い回す」で、RDR は、複数クレートのビルド（KPI の crates edit）で次に大きく効くのがこれだという上流からの裏付けになる。
- **TPDE**（[目標](https://goals.rust-lang.org/2026/tpde.html)、[論文](https://arxiv.org/abs/2505.22610)）。ミュンヘン工科大学のバックエンドで、解析を 1 パス走らせたあと、命令選択、レジスタ割り当て、エンコードを 1 パスでする。LLVM IR から `-O0` の 8〜24 倍の速さで、コードの質は同じくらい（SPECint 2017）。rustc では LLVM バックエンドの中の `-Z tpde` として使えるようにし、対応しないものは `-O0` に戻す。対象は ELF の x86-64 と AArch64 だけで、legalize できないベクトル型が Rust のコードでの既知の障害。zustc にとっては minclif（自前のレジスタ割り当てを持つ小さな x86-64 バックエンド）の方向を裏付けるもの。minclif と Cranelift の `single_pass` 割り当てが時間をどこに使っているかを、TPDE のシングルパスの設計と比べて測る。
- **`rustc_codegen_cranelift` の改善**。バックエンドの中で 2 倍を目標にしている。2026 年 2 月の時点で、Zed のインクリメンタルビルド（デバッグ情報なし）で LLVM より約 5% 速く、行テーブルありの単一クレートで平均 10〜15% 速い。zustc はすでに Cranelift（0.137.1）を使い、`-O` でないときは `regalloc_algorithm=single_pass` にしている（`crates/clif/src/lib.rs`）。Cranelift 自体の改善はクレートを上げれば入る。
- **多相なコード生成の実験**（[目標](https://goals.rust-lang.org/2026/polymorphization.html)）。ジェネリックなコードに型引数ごとの隠れた引数（`TypeId`、drop glue、trait のメソッド）を足し、インスタンスはデータレイアウトでだけ分ける（`foo::<&u32>` と `foo::<&String>` は 1 つを共有する）。単相化の後の処理が 30% から 2 倍速くなり、バイナリも小さくなる見込み。試作は `std` で一部動いている。これはロードマップの「レイアウトをキーに単相化したインスタンスを共有」（blaze の polymorph）にあたる。隠れた引数を足すところは、zustc の計画より一歩先にある。
- **Wild によるインクリメンタルリンク**。まだ目標（「2026 年以降」）で、Wild 自体もまだインクリメンタルにリンクしない。zustc のロードマップの項目（libwild の組み込み、変わった `.o` だけの差し替え、余白とジャンプテーブル）は上流に遅れていない。
- **target ディレクトリを小さくする**。zustc のキャッシュは gen1000 で rustc のインクリメンタルのディレクトリの 1/14（PERFORMANCE.md）。ここから取り入れるものはない。
- **クレートのスライス**（将来の項目: 依存クレートの使われる部分だけをコンパイルする。実験では 1.4〜30 倍）。zustc はプログラムと prelude の中で、すでに `main` から届くものだけをコンパイルしている。
- **並列フロントエンド**: [#101](https://github.com/mizchi/zust/issues/101)。

## 3. 型システムと borrow check

### Polonius Alpha

2026-08-04 から nightly で既定になり、`-Zpolonius=off` で NLL に戻せる。stable は 2026 年末までの予定（[ブログ](https://blog.rust-lang.org/2026/08/04/enabling-polonius-alpha-on-nightly)）。outlives の制約が位置に依存するようになる。返される region に流れる借用は、そこへ流れる経路の上でだけ生きている。完全な Polonius（datalog 版）は予定になく、Alpha はそれの部分集合でも上位集合でもない。

`tests/ui/nll/polonius` のうち補助クレートを使わない 34 件を、stable（NLL）、nightly（Alpha）、`zustc check` で検査した。比べたのは受理か拒否かだけ:

| NLL / Alpha / zustc | 件数 |
|---|---:|
| 拒否 / 受理 / 受理 | 13 |
| 拒否 / 受理 / 拒否 | 2 |
| 受理 / 受理 / 拒否 | 1 |
| 拒否 / 拒否 / 受理 | 2 |
| 判定が同じ | 14 |
| zustc が未対応 | 2 |

zustc は NLL problem case 3（9 件）をすでに受理する。借用は、それを持つ生きたローカルがある間だけ生きているから。ブログのいちばん小さな例（`let b = &mut *a; if true { b } else { a }`）は拒否する。`Bk::forever` が、universal region に流れる借用を本体の最後まで生かすからで、これは NLL の規則。付け替える `&mut` でリストを最後までたどる例は受理する。Alpha は拒否し、datalog の Polonius は受理する。これらのテストはどれも `//@ revisions` を持つので `bench/rust_ui.py` が飛ばしていて、tests/ui の数字にはまったく表れない。詳しい内容と計画: [#1](https://github.com/mizchi/zust/issues/1#issuecomment-7110120344)。

### 次世代の trait solver

1.84 から coherence で、1.100 から既定で orphan check で使われている。2026 年の目標は、どこでも安定させて古い solver を消すこと（[目標](https://goals.rust-lang.org/2026/next-solver.html)）。rust-analyzer は chalk をこれに置き換え、blaze は rustc の solver を vendor し、relax は移植した。

`tests/ui/traits/next-solver/**` の 193 件を、stable（古い solver）、`-Znext-solver=globally` の nightly、`zustc check` で検査した。判定が同じものが 113 件、誤った拒否が 26 件、見逃しが 23 件、新 solver だけが受理して zustc が拒否するものが 14 件、新 solver だけが受理して zustc も受理するものが 9 件、未対応か時間切れが 7 件。誤った拒否の多くは、trait の goal が導くはずの推論（「cannot infer type parameter `T`」）、オブジェクトと param env の射影、候補の優先順位。この実行の途中でメモリ不足のためにプロセスが 1 つ kill されたので、件数は数件ずれているかもしれない。計画: [#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011)。

### 2026 年の目標にあるほかのもの

まだ取り上げていないが、どれも zustc が検査するものに関わる: "Next-Gen Region Constraints"、"Expanding a-mir-formality to work better as a Rust type system spec"（region check の答え合わせに使えるかもしれない）、"End-to-End Executable Rust Specification"、"Stabilize FLS Release Cadence"、"Reintroduce a FCW system to borrowck"、"Normative Documentation for Sound `unsafe` Rust"。

## 4. 言語とライブラリ: 1.95〜1.100

前回の調査からの stable: 1.95（2026-04-16）、1.96（2026-05-28）、1.97（2026-07-09）、1.98（2026-08-20）、1.99（2026-10-01）、1.100（2026-11-12）。1.101 は beta で、2026-12-24 の予定。

リリースごとに機能を 1 つ使うプログラムを書き、rustc 1.97、rustc 1.102-nightly、zustc（`--edition=2024`）でビルドして実行した。1.97 より後に安定した機能は、nightly の出力と比べた:

| リリース | 機能 | zustc |
|---|---|---|
| 1.95 | `cfg_select!` | 通る |
| 1.95 | `if let` ガード | 通る |
| 1.95 | `AtomicUsize::update` | 通る |
| 1.96 | `core::range::Range`（Copy、IntoIterator） | 「cannot find value `from`」 |
| 1.97 | `u32::bit_width` | メソッドがない |
| 1.98 | `#[core::derive(Debug, Clone)]` | **属性が黙って捨てられ**、そのあと「no method named `clone`」 |
| 1.98 | `str::substr_range`、`bool::ok_or`、`Path::is_empty` | `substr_range` がない |
| 1.99 | `Vec::into_parts` / `from_parts`、`String::from_utf8_lossy_owned` | `into_parts` がない |
| 1.100 | 型としての `!`（`Vec<!>`、`Result<u8, !>`、反駁できない `let Ok(x)`） | 通る |
| 1.100 | `Infallible` が `!` の別名 | 「expected `Result<i32, !>`, found `Result<i32, Infallible>`」 |
| 1.100 | `Allocator` / `Global`、`Vec::new_in`、`Box::new_in` | 「cannot find type `Global`」 |
| 1.100 | `bool::toggle`、`Vec::from_fn`、`str::trim_prefix` | `toggle` がない |

C の可変長引数の関数の定義（1.99）も試したが、試したプログラムが nightly にない `VaList` のメソッドを使っていたので除いた。

このほか、zustc が追うべきこれらのリリースの変更:

- 1.100: never 型のフォールバックがすべてのエディションで `!` になった。`box` パターンが使えなくなった。orphan check で新 solver。std の Unicode が 18.0.0。
- 1.99: 属性マクロと derive マクロの中の `mod foo;`（#29 と関わる）。`no_mangle_generic_items` がハードエラー。
- 1.98: 曖昧な import のエラーが厳しくなった（#79）。`repr(transparent)` の変更。
- 1.97: v0 シンボルマングリングが既定（zustc のオブジェクトが rustc のものと混ざるところでだけ関わる）。
- 1.95: 独自の JSON ターゲット指定が stable で使えなくなった。

`#[core::derive]` の行は、TODO.md の「zustc 単体では未知の属性を黙って捨てる」と同じ種類の問題。zustc の知らない属性のパスは、何もしないのではなくエラーにすべき。

次に来るもの（2026 年の目標、nightly）: リフレクション（`TypeId::info`。定数評価だけの MVP が入った）、return type notation と TAIT、`dyn` trait の async fn、フィールドの射影、その場での初期化、明示的な末尾呼び出しと `loop_match`、open enum、ジェネリックな整数型、`Try` trait、（最小の）特殊化、参照カウントを書きやすくする機能。

## 5. 意見が割れているところ

- **参照カウントを書きやすくする機能**。RFC 3680 の `.use` と `Use` trait は評価が割れた。2026 年の目標はこれを `Share` trait や `move($expr)` のクロージャと比べ、「カーネルに使えるほど低水準で、GUI に使えるほど書きやすい」ものを目指している。割れているのは、必須の構文を増やすことがそもそも助けになるのかという点。どの設計もまだ採られていない（[目標](https://goals.rust-lang.org/2026/ergonomic-rc.html)）。
- **Polonius Alpha と datalog の Polonius**。Alpha は、datalog の Polonius が受理していたプログラム（付け替える `&mut` のループ）をあきらめていて、上流は Alpha の後の作業を予定していない。zustc はいま、それらのプログラムで datalog 側にいる。rustc に合わせるなら、それらを拒否することになる。
- **RDR の優先度**。RDR は資金のない項目とされ、2025H2 には「完了しない」とされた。10 月の目標と open な PR より前の 2026 年初めには、進んでいないのではと疑う声があった。
- **リフレクション**。範囲（まず定数評価の値だけにし、型を型システムに戻すのは後）と API の名前（目標の `TypeId::info` と、ほかで報告されている `type_info`）が決まっていない。
- **サプライチェーン**。
  - 2026 年 9 月に Rust のブログが、rust-lang のメンバーと人気のクレートの所有者を狙った攻撃を警告し、`arrayref` が一時乗っ取られた。
  - 1.100 で Cargo に公開からの最短経過期間（依存のクールダウン）の設定が入り、"crates.io hardening against supply chain attacks" が 2026 年の目標になっている。
  - zurgo は crates.io のソースを読むので、Cargo のクールダウンの設定に従うべき。
- **コンパイラの外**: Ubuntu の Rust 版 coreutils（uutils）への移行。ライセンス（MIT と GPL）、互換性のバグ、変更の速さ。

## 6. することと順番

1. [#101](https://github.com/mizchi/zust/issues/101): `check` の並列化。新しい解析は要らず、cold の check でほぼコア数ぶん効き、危ないところ（決定性、KPI の数え方）も分かっている。
2. [#1](https://github.com/mizchi/zust/issues/1): universal region に流れる借用の、位置に依存する生存解析と、Polonius のテストを比較に入れること。stable の rustc は 2026 年末までに Alpha になる予定。
3. 4 節の 1.96〜1.100 の穴。prelude と属性の小さな作業: 未知の属性のパスをエラーにする、`core::derive`、`Infallible = !`、`Allocator` / `Global`、新しい API、`core::range`。
4. [#2](https://github.com/mizchi/zust/issues/2): trait solver を issue の 6 段階で。tests/ui でいちばん大きな分野で、rust-analyzer、blaze、relax がすでに進んだ方向。
5. 解析結果のクレートをまたいだ使い回し（zustc の RDR）。
6. レイアウトをキーにしたインスタンスの共有。上流の polymorphization の実験を参考にする。

## 7. 追い続けるために

- rustc のリリースごと（6 週間ごと）に、そのリリースで安定したものについて 4 節の確かめ方を繰り返す。このリポジトリが rustc を上げるときに tests/ui の固定する版も上げる。
- `bench/rust_ui.py` は `//@ revisions` を持つテストをすべて飛ばすが、上流の新しい振る舞いはそこで試されている（`nll/polonius`、`traits/next-solver`）。rust_ui.py が revision を選べるようになるまでは、それらのディレクトリをこのレポートの 3 つのコンパイラで比べる。
- Rust のブログの 2026 年の目標の月次報告で、RDR、TPDE、並列フロントエンド、Polonius Alpha、次世代 solver、polymorphization を追う。

## 出典

- [Rust Project Goals 2026](https://goals.rust-lang.org/2026/goals.html)、[Fast Builds のロードマップ](https://goals.rust-lang.org/2026/roadmap-fast-builds.html)
- [Relink Don't Rebuild の tracking issue](https://github.com/rust-lang/rust/issues/158844)、[goals #722](https://github.com/rust-lang/goals/pull/722)
- [TPDE の目標](https://goals.rust-lang.org/2026/tpde.html)、[TPDE の論文](https://arxiv.org/abs/2505.22610)、[TPDE のリポジトリ](https://github.com/tpde2/tpde)
- [多相なコード生成の実験](https://goals.rust-lang.org/2026/polymorphization.html)
- [cg_clif の性能改善](https://goals.rust-lang.org/2026/improve-cg_clif-performance.html)
- [並列フロントエンドの目標](https://goals.rust-lang.org/2026/parallel-front-end.html)、[Phoronix: nightly で並列フロントエンドが既定に](https://www.phoronix.com/news/Rust-Nightly-Parallel-Frontend)
- [Polonius Alpha を nightly で有効にする](https://blog.rust-lang.org/2026/08/04/enabling-polonius-alpha-on-nightly)、[Polonius の目標](https://goals.rust-lang.org/2026/polonius.html)
- [次世代 trait solver の目標](https://goals.rust-lang.org/2026/next-solver.html)
- [リフレクションと comptime の目標](https://goals.rust-lang.org/2026/reflection-and-comptime.html)、[参照カウントを書きやすくする目標](https://goals.rust-lang.org/2026/ergonomic-rc.html)、[RTN の目標](https://goals.rust-lang.org/2026/rtn.html)
- リリースノート: [1.95](https://blog.rust-lang.org/2026/04/16/Rust-1.95.0/)、[1.96](https://blog.rust-lang.org/2026/05/28/Rust-1.96.0/)、[1.97](https://blog.rust-lang.org/2026/07/09/Rust-1.97.0/)、[1.98](https://releases.rs/docs/1.98.0/)、[1.99](https://releases.rs/docs/1.99.0/)、[1.100](https://releases.rs/docs/1.100.0/)
- [Be alert: targeted attacks on prominent Rustaceans](https://blog.rust-lang.org/2026/09/17/targeted-attacks/)
- [The acyclic e-graph: Cranelift's mid-end optimizer](https://cfallin.org/blog/2026/04/09/aegraph/)
