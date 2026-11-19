# 形式モデルと有界な証明: 借用検査・スレッドモデル・アロケータ

Date: 2026-11-19

zustc の重要な部分（借用検査、並行性、`Vec` のメモリ管理）を、[idealis](https://github.com/mizchi/idealis)（TLA+ の有限モデル検査を Rust で行う: rtla → rtla-compiler → rtlc）と [Kani](https://github.com/model-checking/kani)（CBMC による Rust の有界モデル検査）で形式化した。モデルとハーネスは [formal/](../../../formal/README.md) にあり、`formal/run.sh` で再実行できる。

この作業で、次の 5 つの欠陥が見つかった。

1. 借用検査が不動点の回数上限で打ち切ると、誤りなしとして受け付ける。
2. `Vec::with_capacity` の容量計算が桁あふれし、安全なコードがヒープを壊す。
3. `realloc` の失敗（null）を検査していない。
4. キャッシュの一時ファイル名を全プロセスで共有しているため、混ざったファイルを受け付けうる。
5. `std::thread::spawn` を即時実行にしているので、意味が rustc と違う。

zustc のコードは変えていない（修正案は各節にある）。

## 先行研究

| 分野 | 先行研究 | ここでの使い方 |
|---|---|---|
| 借用検査 | [Wu, Wang, Chen: Verified Implementation of the Next-gen Borrow Checking Algorithm for Rust via Abstract Interpretation (arXiv 2611.06319, 2026)](https://arxiv.org/abs/2611.06319): Polonius を前向きの抽象解釈として定式化し、Rocq で健全性を証明 | zustc の設計（CFG 上の前向き解析、origin はローカル変数ごとに 1 つ）に最も近い。領域の束と単調性を述べる型紙 |
| | [RustCompCert (arXiv 2602.07455, 2026)](https://arxiv.org/abs/2602.07455): Polonius を Kildall の枠組みの transfer 関数として実装し、領域が半束であることを証明 | 不動点が定義され停止することの証明の型紙。本報告の束の検査はその有限版 |
| | [Polonius の規則](https://rust-lang.github.io/polonius/)、[Oxide (arXiv 1903.00982)](https://arxiv.org/abs/1903.00982)、RustBelt (POPL 2018) | 参照する仕様。小さな CFG での差分テストのオラクルの候補 |
| スレッド | RustBelt の Send/Sync、RbRlx (POPL 2020)、[RustMC / GenMC (arXiv 2502.06293)](https://arxiv.org/abs/2502.06293)、Loom、Shuttle | Kani は逐次実行だけを扱う（[Kani の論文 arXiv 2607.01504](https://arxiv.org/abs/2607.01504) とドキュメント）。そのため並行性は TLA+ で扱う |
| ビルドとキャッシュ | Build systems à la carte (ICFP 2018)、[Forward Build Systems, Formally (CPP 2022)](https://arxiv.org/abs/2202.05328)、[CloudMake (FM 2014)](https://www.microsoft.com/en-us/research/wp-content/uploads/2014/05/krml235.pdf) | 「キャッシュから読んだ結果は、作り直した結果と同じ」が正しさの定義。fork/pipe のワーカーや、内容でアドレスするキャッシュの保存手順を TLA+ で書いた例は見つからなかった |
| アロケータ | [verify-rust-std](https://github.com/model-checking/verify-rust-std)（challenge 19: RawVec は VeriFast で解決済み、23/24: Vec は未解決）、[RefinedRust (PLDI 2024)](https://plv.mpi-sws.org/refinedrust)、[verified-memory-allocator (Verus)](https://github.com/verus-lang/verified-memory-allocator)、Gillian-Rust (PLDI 2025) | verify-rust-std の境界外アクセス・不正な deref・未初期化読み出しのハーネスを、prelude の `Vec` 用の型紙にした |
| 仕様と実装の接続 | quint-connect、tla-connect、Using Lightweight Formal Methods to Validate a Key-Value Storage Node in Amazon S3 (SOSP 2021) | 次の一手（仕様のトレースを実装で再生する） |

調査では、各文献の URL と要旨を確かめた。RustBelt・RbRlx・Build systems à la carte・S3 の論文は、よく知られた文献として記憶から挙げた。

## 1. 借用検査（ライフタイムの解析）

[報告 08](2026-11-18-08-borrow-checking.md) の解析は、ブロックの入口の状態を `State::join` で大きくしていく worklist 不動点である。これが正しいには、束の演算について次が成り立つ必要がある。

- ソート済みの集合が、ソート済みで重複なしのまま保たれる（`binary_search` がそれを前提にしている）。
- join の結果がちょうど両辺の和集合になる。
- 集合として変わったときは、必ず「変わった」と報告する。
- 同じものをもう一度 join しても何も変わらない（ループが止まる）。
- 場所の順序（`prefix`）が反射的・推移的・反対称であり、`overlap` が対称である。

`crates/typeck/src/acg/borrowck_props.rs` は、これを小さな宇宙で全数検査する（`cargo test -p zust-typeck props`、0.04 秒）。

- `add_sorted` は、0..6 のすべての集合とすべての値で調べる。
- `join` は、5 つの成分それぞれのすべての値の組で調べる（1 万組以上）。
  - `join` は成分ごとに独立したループで、変更の印はその論理和なので、成分ごとの全数で全体を覆う。
  - holds では、局所変数がない場合・空の場合・順序が逆の場合も含める。
- 場所の順序は、長さ 2 までのすべての経路の組と 3 つ組で調べる。

すべて成り立った。

最初はこれを Kani で書いた。しかし CBMC は、要素 4 個の `Vec<u32>` に記号的な位置で `insert` するだけでメモリを使い切った（約 15 GiB、命題への変換の段階）。同じ関数を単独の crate に移しても同じだった。`State` のように `Vec` を入れ子にした値の性質には、Kani は向かなかった。

**欠陥 1: 不動点の打ち切りが誤りを見逃す。** `Bk::check` は worklist の回数が `50 * ブロック数 + 1000` を超えると `return None`（誤りなし）を返す。コメントには「起きないはず: 状態は大きくなるだけ」とある。

- 停止すること自体は正しい。join は単調で、束の高さは有限だからである。
- しかし回数は、束の高さとブロック数の積で増える。上限はブロック数にしか比例しない。
- 再現は `formal/repro/borrowck_rounds.py K` で行う。K 個の参照をループの中で逆順に 1 つずつずらすと、1 周ごとにローンが 1 段進む。その後で `x = 1` と書く。
  - rustc はどの K でも E0506 を出す。
  - zustc は K = 259 までは見つけ、K = 260 からは見逃して受け付ける。

修正案:

- 上限を、束の高さ H（ムーブした場所・ローカル変数・ローン × (ローカル変数 + 2) の個数）を使った、証明できる上限 `n * (H + 1) + 1` に変える。
- または上限を超えたら受け付けずに誤りにする。
- 逆後順（reverse postorder）の worklist にすれば、回数自体も減る。

## 2. スレッドモデル

### zustc の実行時

prelude の `std::thread::spawn` は、クロージャをその場で実行する。`Arc` は `Rc` と同じで、atomic はただのセルである。スレッドが 1 本なので `Arc` の参照数は安全だが、意味の違いが 2 つある（`formal/repro/`）。

- `thread_spawn_channel.rs`: 子が `rx.recv()` し、親が spawn の後で `send` する。rustc は `child got 42` を出す。zustc では、子が空のチャネルで `recv` するので `Err` になり、panic する。即時実行は Rust の実行順の 1 つでしかない。子が、親が spawn の後に行うことを待つと成り立たない。
- `thread_spawn_mutex.rs`: 親がロックを持ったまま spawn し、子もロックする。rustc は親が解放するまで待つ。zustc では子がそのままロックを取り、1 を出す。結果は同じだが、相互排他は破られている（`Mutex` はブロックしない）。

`formal/tla/ArcDrop.tla` は、本物のスレッドを入れる前に通すべき検査である。2 スレッドがそれぞれ clone を drop する。

- prelude の今の `strong -= 1; if strong == 0 { free }`（読みと書きが別）では、rtlc が 7 手で反例を出した。clone の書き込みが drop の書き込みに上書きされ、clone をまだ持っているのに解放される（use after free）。
- `fetch_sub(1) == 1` では全探索で成り立つ（逐次一貫性の下。Release/Acquire の議論は RbRlx の範囲で、このモデルの外）。

### コンパイラ自身の並行性

zustc はスレッドを使わない。並行して走るのは、fork した子プロセスと、同じ出力に対して同時に走る別の zustc プロセスである。

`formal/tla/ForkPool.tla`（driver2.rs の要約、build.rs の関数とクレート）は、次の起こりうることをモデル化した。

- fork の失敗。
- 子がどこでも死ぬこと、またはレコードの途中で書き込みが止まること（シグナル、短い write）。
- pipe が満杯なら、書く側が待つこと。
- 子は、fork の時点で親が持っている pipe の書き込み端をすべて受け継ぐこと。

その上で、親が受け取る値が正しいこと（NoWrongValue）、最後にすべての項目が正しいこと（Correct）、デッドロックがないことを検査した。

- 今のコードの順序（子 k を fork した直後に、親が pipe k の書き込み端を閉じる）では、子 2 人・項目 4（257 状態）でも、子 3 人・項目 6（1862 状態）でも成り立つ。
- 対照として、すべての子を fork した後で閉じる版も検査した。子 2 が pipe 1 の書き込み端を持ったまま pipe 2 の空きを待ち、親が pipe 1 の EOF を待つ。rtlc はこのデッドロックを 6 手で見つけた。今のコードが、まさにこの落とし穴を避けていることの確認になる。

**欠陥 4: キャッシュの保存。** `formal/tla/CacheSave.tla`。`Cache2::save` は `<path>.tmp` に書いてから rename する。`Cache2::open` は、ヘッダと長さが範囲内なら受け付ける（チェックサムはない）。2 つの zustc が同じ出力に同時に保存する（ビルドと `zustc check` を並べて走らせるなど）と、次のことが起きる。

- 一時ファイル名が同じなので、B が A の書きかけの tmp を O_TRUNC で開き直す。
- A が rename した後も、B はそのファイル（もう `<path>`）に書き続ける。
- 読み手は、B のヘッダと A の残りが混ざった、構造としては正しいファイルを受け付けうる。rtlc は 8 手の反例を出した。
- このときエントリは B のキーを持ち、中身の一部が A のバイトになる。間違った機械語がキャッシュから出る可能性がある。
- また B の rename は失敗する（警告が出るだけ）。

窓は狭いが、POSIX の上では起こりうる。一時ファイル名をプロセスごとに分ける（`<path>.<pid>.tmp`）と、全探索で成り立つ（72 状態）。保存するファイルにチェックサムを入れれば、壊れた読み込みも捨てられる。

あわせて、コードを読んで次の点を確かめた（モデルには入れていない）。

- `read_all` / `write_all` は、EINTR でも打ち切る。どちらも親が作り直す経路に落ちるので、結果は正しい（遅くなるだけ）。
- build.rs の関数のコンパイルでは、fork や pipe が失敗すると、すでに fork した子を待たずに `Err` を返す。

## 3. アロケータ（prelude の `Vec`）

`formal/kani-prelude` の `build.rs` は、`crates/syntax/src/prelude_lib.rs.in` から `Vec<T>` とその 16 個のメソッド、`Drop` をそのまま写す。こうすると、zustc がコンパイルするコードそのものを検査できる。prelude が変わればハーネスも変わる。

zustc が与えるものはモデルにした。

- `realloc` / `free` は、ブロックが自分の大きさを知っているモデルにした。`realloc` は `min(旧, 新)` バイトを写し、許せば null を返す。
- `__exr_drop_in_place` は T の drop にした。

検証できたこと（容量 4 まで、`with_capacity(0..4)` の後に 0..3 回 push した任意の状態から）:

- `push`・`pop`・`insert`（位置 0/1/2 ごと）・`remove`・`swap_remove`・`truncate`・`reverse` は、どれも配列の参照モデルと同じ中身になる。
- `len <= cap` が保たれる。
- 割り当てた範囲の外に触れない（Kani のポインタ検査）。
- drop がすべてのブロックを解放する。
- `drops_each_element_once`: どの操作の後でも、各要素はちょうど 1 回 drop される。

見つかったこと:

- **欠陥 2:** `bug_with_capacity_overflow`。`reserve` の `c * size_of::<T>()`（と `len + extra`）が桁あふれする。
  - 小さなブロックなのに `cap` が大きいままになり、push がブロックの外に書く。
  - `formal/repro/vec_capacity_overflow.rs`（`Vec::<u64>::with_capacity(1 << 61)` の後に push）は、zustc では segmentation fault になる。rustc は "capacity overflow" で panic する。
  - 安全なコードでヒープが壊れる。修正案は `checked_add` / `checked_mul` で、失敗したら panic する。
- **欠陥 3:** `bug_alloc_failure_unchecked`。`realloc` が null を返しても `reserve` はそれを使い、push が null に書く。std は `handle_alloc_error` で abort する。

Kani の限界:

- 参照モデルに std の `Vec` を使うと、CBMC がメモリを使い切った。固定長の配列にした。
- 記号的な位置での `insert` も、メモリを使い切った。位置ごとのハーネスに分けた。
- 証明は容量 4 までの有界なものである。任意の長さへは、Kani のループ契約（`-Z loop-contracts`）か、Verus・RefinedRust のような演繹的な検証が要る。

## 再実行

```bash
formal/run.sh tla
```

```bash
formal/run.sh kani
```

```bash
cargo test -p zust-typeck props
```

TLA+ の 7 構成はそれぞれ 1 ms 前後で終わる。Kani の 12 ハーネスは約 8 分かかり（10 が検証され、`bug_*` の 2 つは期待どおり失敗する）、5〜10 GiB を使う。idealis は git の rev（89f54a9）で固定している。

## 次の一手

- 欠陥 1〜4 の修正。借用検査の上限を証明できる式にし、`Vec` の桁あふれと null を検査し、キャッシュの tmp 名をプロセスごとにする。
- 借用検査の差分テスト。小さな CFG とイベント列を全数生成し、Polonius の規則を経路ごとに素直に実装した参照と比べる。gen/kill が分配的な部分では、経路ごとの結果を合わせたもの（MOP）と不動点（MFP）が一致するはずである。
- 仕様と実装の接続。ForkPool / CacheSave のトレースを、quint-connect のように実際の driver2 で再生する。idealis の Driver / `idealis-testing` を使う。
- スレッドを入れるときは、`ArcDrop.tla` を atomic 版で通すことと、`spawn` の即時実行をやめることが前提になる。
