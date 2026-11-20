# リージョン (ライフタイム) 検査の方針と手順 (issue #1)

日付: 2026-11-20

## 1. 現状の計測

- 次の数え方はすべて callgrind による命令数。
- **gen5000 の `check`: 3793M。** 内訳は次のとおり (包含)。
  - 関数ごとの acg (`compile_id_req`): 60%
  - `analyze` (構文解析・要約・prep): 35%
  - `Bk::check`: 0.1% 未満。前段の、位置を見ない検査で打ち切られている。
- **patterns の `check`: 685M。** 内訳は次のとおり。
  - `compile_id_req`: 56%
  - `Bk::check`: 2.7%
  - `bk_scrutinee`: 1.1%
  - borrowck 全体の費用は README にある「+2.2%」と同じ程度。
- **check の作り:**
  - 波 (wave) ごとに関数を 1 つずつ逐次に処理する。
  - 関数ごとの結果は check memo (`driver2.rs`) とキャッシュに残るので、関数 1 つの編集では再検査はその関数だけで済む。
  - build は、fork した子プロセスで並列にしている (`compile_many`)。
- **型とシグネチャのライフタイム:**
  - `Ty` はリージョンを持たない。
  - シグネチャのライフタイムで残るのは `SigSyn::ret_from` だけ。戻り値がどの引数を借りるかを示すビットマスク。
  - そのため **iface 指紋 (`iface_fp`) はライフタイムを含まない**。たとえば `-> &'a T` を `-> &'static T` にしても、呼び出し側は再検査されない。
  - これは今は問題にならない (検査しないから) が、リージョン検査を入れると正しさの問題になる。
- **tests/ui の compile-fail で、リージョン関係の注記があって今は通らないもの:**

| 分類 | 不通過 (fail + 行違い) | ADT のライフタイム・HRTB・クロージャ・dyn/impl なしの数 |
|---|---:|---:|
| lifetime may not live long enough | 189 | 44 |
| does not live long enough (E0597) | 89 | 30 |
| 一時値 (E0716) | 55 | 37 |
| `T: 'a` (E0310/E0309) | 50 | 10 |
| E0621/E0623/E0700 など | 24 | 3 |
| E0515 / E0373 / E0478 / E0521 / E0713 | 25 / 12 / 11 / 1 / 3 | - |
| 構文だけで分かるもの (E0261 未宣言, E0106 欠落, E0195/E0207/E0263/E0496 など) | 18 / 21 / 31 | - |

- 表から次の 2 点が読み取れる。
  - ADT のライフタイム引数 (`struct S<'a>`) を扱わないと、取れるのは全体の 1/4 程度。
  - 構文だけの検査が、費用ほぼ 0 で ~70 件ある。

## 2. 最近の研究と実装からの示唆

- **NLL (rustc) の universal region の検査:**
  - リージョンの値は「CFG の点の集合 + end('a)」。
  - `'a: 'b` 制約を SCC にまとめて伝播し、`check_universal_regions` で、宣言にない `'a: 'b` が必要になっていないかを見る。
  - universal region はもともと関数本体のすべての点を含む。そのため、シグネチャに対する検査 (「lifetime may not live long enough」など) は、**位置を見ない到達可能性だけで決まる**。
  - [rustc-dev-guide: region inference](https://rustc-dev-guide.rust-lang.org/borrow-check/region-inference.html)
- **Polonius alpha (2025H2〜2026 の project goal):**
  - 節点が (リージョン, 点) の制約グラフ上で、ローンが生きているかを到達可能性で求める。
  - グラフは実体化しない (**lazy**)。どこでも成り立つ制約は「論理辺」として持ち、たどるときに展開する。リージョンの liveness も、たどって初めて触れたときに計算する。
  - ローンがない関数ではグラフを作らない。ローン数はベンチの 90% で 1 桁。
  - 目標は「NLL 比 +10〜20% まで許容」。
  - [goals 2026/polonius](https://goals.rust-lang.org/2026/polonius.html)
  - [rustc_borrowck::polonius](https://doc.rust-lang.org/nightly/nightly-rustc/src/rustc_borrowck/polonius/mod.rs.html)
- **zust の borrowck との関係:**
  - すでに Polonius 流で、origin を局所変数ごとに 1 つ持ち、ローンが生きているかを局所変数の liveness で決めている。
  - 足りないのは **universal region (シグネチャ側) と、ADT のライフタイム引数** だけ。

## 3. 性能上のボトルネックの仮説

- **H1 費用の大半は制約の「収集」で、「解く」ほうではない。**
  - acg は式を全部たどるので、制約を収集するフックの費用は式の数に比例する。
  - 解く側は関数ごとの小さいグラフ (SCC と到達可能性、O(辺)) で済む。
  - → 収集はゲートで止める (H2)。解くのは、関数の終わりに 1 回だけにする。
- **H2 リージョンを持つ型がない関数は、0 の費用にできる。**
  - シグネチャにも局所変数の型にも参照やライフタイム引数を持つ ADT がなければ、universal region の検査は要らない。この判定は要約の時点で一度だけ行う。
  - gen5000 と crates はほぼ全部ゲートで落ち、patterns と traits が費用を払う。
- **H3 `Ty` にリージョンを足すと、全 KPI が悪くなる。**
  - `Ty` は、インスタンスのキー・キャッシュの符号化・ハッシュに使われている。
  - そこで rustc の「codegen では erased」と同じく、`Ty` は消去したまま、検査のときだけ **リージョンの「影」(region slots) を横に持つ**。
  - region slots は型の中の参照と、ADT のライフタイム引数の位置の列。
  - → no-op build・キャッシュ量・edit の KPI は、原理的に動かない。
- **H4 universal region の誤りは、位置を見ない検査で足りる。**
  - これで取れるのは may-not-live、E0310、E0621、E0478、`'static` が要る E0597 の大半。
  - 位置が要るのは、局所のローン (既存の borrowck) と、一時値 (E0716、文の終わりで死ぬ隠れ局所) だけ。どちらも既存のデータフローに乗る。
- **H5 ADT の変位 (variance) は要約の段階で一度だけ計算し、キャッシュする。**
  - 費用は cold の時だけで、ADT の数に比例する。
- **H6 リージョンのシグネチャは `SigSyn` に入れる。**
  - 入れないと、増分ビルドが不正確になる (§1)。
  - ライフタイムのない fn では空にして、要約とキャッシュの大きさを増やさない。
- **H7 LSP は関数ごとの memo に乗る。**
  - 1 回の編集で増える費用は、その関数のリージョン検査だけ。

### マルチスレッドの別経路について

- **候補 A: リージョン検査を別スレッドで解く。**
  - 解く部分は全体の数% (H1) なので、スレッドの受け渡しの費用のほうが大きい。採らない。
- **候補 B: check の関数ごとの acg を、`std::thread::scope` で並列にする。**
  - 波の中の関数は独立している。`Insts` はもう `Mutex`。状態は thread_local だけ。
  - 効くのは cold の check と LSP の初回、つまり実時間 (vs_blaze)。
  - cachegrind は全スレッドの命令を数えるので、命令数の KPI は良くならない。
  - そこで、リージョン検査とは独立した経路として最後に置き、受け入れ基準 1 (blaze より速い) の余裕が減ったときだけ行う。

## 4. 方針

段階ごとに、計測してから残す。どれも、誤りを出すのは確実な場合だけにする。精度が足りない形なら、黙って受け入れる。run-pass と check-pass の退行は 0 にする。

- **R0 基盤**
  - リージョンのシグネチャ (`SigSyn` へ。iface 指紋と増分ビルドの正しさのため)。
  - ADT のライフタイム引数・各フィールドの region slots・変位を要約に入れる。
  - ゲートの印。
- **R1 構文だけの検査** (要約の時点、費用ほぼ 0): E0261、E0106、E0263/E0496/E0195 など。
- **R2 universal region の検査** (位置を見ない、関数ごと):
  - 局所ごとに origin 1 つ、という今の粒度では足りない。
    - `fn foo(x: &mut Vec<&u8>, y: &u8) { x.push(y) }` は、`Vec<T>::push` の `T` を通して `'y: 'x の要素` が要る (`&mut` の下なので不変)。
    - tests/ui の典型はこの形。
  - そこで、ゲートを通った関数の中だけ、**式の値ごとに region tree** を作る。
    - region tree は、型の region slots の並びにリージョン変数を置いたもの。
    - 呼び出しでは、callee の汎用シグネチャを新しい変数で具体化する。引数の tree との間に、変位に従って outlives 制約を張る。
    - 分からない構文は Unknown にする。Unknown に触れる制約は捨てる。こうして、誤検出を出さない。
  - SCC と到達可能性を求め、宣言にも暗黙の境界にもない `'a: 'b` が要れば、「lifetime may not live long enough」を出す。
  - 局所のローンが `'static` / universal に流れたら E0597 を出す。
- **R3 一時値 (E0716):** 文の終わりで死ぬ隠れ局所を置く。既存のデータフローに乗せる。
- **R4 ジェネリクスの `T: 'a` (E0310/E0309)、`impl` のライフタイム、クロージャ・HRTB の一部。**
- **R5 (必要なときだけ)** check の関数ごとの並列化 (候補 B)。

## 5. 1 段ごとの手順

1. 対象の tests/ui を `--filter` で集め、小さい再現を tests/ に置く。誤りを出すもの、出してはいけないものの両方を置く。
2. 実装する。
3. 次を確認する。
   - `cargo test --release`
   - `python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --check`。退行 0 にする。
   - compile-fail の増分。新しく通るものと、`bench/rust_ui_accidental.txt` から外せるもの。
4. `python3 bench/kpi_track.py "label" --dry-run` を流し、マイルストーンでは `--lsp` も流す。前の行と比べて、動いた分の理由を書く。
5. 費用が仮説を外れたら、callgrind で原因を見る。
6. commit・push する。
