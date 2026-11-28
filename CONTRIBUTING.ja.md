# 開発のしかた

[English](CONTRIBUTING.md) | 日本語

zust (zustc と zust-analyzer) を直す・足すときの手順。使い方は [README.md](README.md)、中のしくみは
[docs/ja/architecture.md](docs/ja/architecture.md)。

## はじめに

clone したばかりのところから、順に行う。終わると、この文書のコマンドがすべて動く。

1. ツールを入れる (Linux x86-64):
   - edition 2024 に対応した Rust のツールチェイン (rustup の stable) と、zust-analyzer が std の定義を引くための
     rust-src (`rustup component add rust-src`)
   - `cc` (リンクの行を組み立てる) とリンカ。zustc は wild > mold > ld.lld > ld の順に選ぶ。ベンチマークの数字と CI の
     サイズ予算は wild 0.11.0 で測っている (`cargo install --locked wild-linker --version 0.11.0`、またはリリースの
     バイナリ)
   - python3 (`bench/` と `scripts/` のスクリプト)
   - valgrind: `tests/compare.sh` がメモリエラーとリークを見て、`bench/kpi_track.py` が cachegrind で命令数を数える
   - あれば:
     - rust-analyzer (`bench/lsp_parity.py`、`bench/ide_golden_ra.py`)
     - cargo-mutants (ミューテーションテスト)
     - cargo-nextest (テストを速く回す。「テスト」を参照。CI はこれで回す)
     - blaze (`bench/vs_blaze.py`)
     - Node 24 以上と TypeSafe の API キー (jev-lint。「英語の文書と日本語訳」を参照)
2. submodule ごと clone する。`cratebench/` と `bench/deps_bench.py` は、そこにある実在のクレートをビルドする。

   ```bash
   git clone --recurse-submodules https://github.com/mizchi/zust.git
   ```

   すでにある clone では `git submodule update --init cratebench/vendor` を実行する。
3. release ビルドの 2 つのバイナリ、`target/release/zustc` と `target/release/zust-analyzer` を作る。テストや
   スクリプトの多くはこの 2 つを使う。

   ```bash
   cargo build --release --workspace
   ```

4. rustc の UI テストを取ってくる。`tests/ui` と `tests/auxiliary` だけの sparse clone で、約 170 MB が `out/rust-tests` に入る。

   ```bash
   python3 bench/rust_ui.py --fetch
   ```

   `tests/auxiliary` を加える前に取ってきたチェックアウトには、一度だけ追加する。compiletest がすべてのテストにリンクする
   C のヘルパー (`rust_test_helpers.c`) が入っている。

   ```bash
   git -C out/rust-tests sparse-checkout add tests/auxiliary
   ```

5. pre-commit フックを有効にする。英語と日本語の文書の食い違いを防ぐ。

   ```bash
   git config core.hooksPath .githooks
   ```

6. 何かを変える前に、すべて通ることを確かめる。

   ```bash
   cargo test --release --workspace
   ```

   ```bash
   tests/compare.sh
   ```

   ```bash
   python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --check
   ```

7. KPI を書き込まずに一度取り、このマシンでの数字を見ておく。命令数はマシンの負荷によらないので、記録と比べられる。

   ```bash
   python3 bench/kpi_track.py "baseline" --dry-run
   ```

## リポジトリの構成

| 場所 | 中身 |
|---|---|
| `src/` | `zustc` の CLI (`src/main.rs`) と、下の crate の再輸出 |
| `crates/base` → `syntax` → `sema` → `typeck` → `codegen` → `core` | コンパイラの層 (下から順に依存する) |
| `crates/minclif` / `crates/clif` / `crates/ir` | 自前の x86-64 バックエンド / Cranelift のバックエンド / 命令の列の型 |
| `crates/ide` | エディタ向けの解析 (シンボル・定義・参照・hover・補完)。コンパイラ本体からは使わない |
| `crates/analyzer` | LSP サーバ `zust-analyzer` |
| `examples/` | zustc でコンパイルする入力 (Cargo の example ではない) |
| `tests/` | zustc のテスト (rustc と比べるもの、キャッシュ、セルフホストなど) |
| `benches/` / `bench/` | criterion のベンチ / ベンチマークと回帰監視のスクリプト |
| `cratebench/` | 実在の小さなクレートをビルドするベンチ |
| `docs/` | 言語のサブセット、設計、作業のレポート (英語。日本語版は `docs/ja/`) |

crate ごとの詳しい中身は [docs/ja/architecture.md](docs/ja/architecture.md) にある。

## テスト

```bash
cargo test --release --workspace
```

全部のテスト。[cargo-nextest](https://nexte.st/) なら、同じテストが 2/3 ほどの時間で終わる (テストごとに別のプロセスで、
テストのバイナリを並べて動かす。4 コアで 71 秒 → 50 秒)。doc テストは動かさないので、CI はそれを `cargo test --doc` で動かす。

```bash
cargo nextest run --release --workspace
```

設定は `.config/nextest.toml` にある (`ci` プロファイル: 最初の失敗で止めない、止まったテストは 5 分で打ち切る)。
多くのプログラムをビルドして動かすテストは、自分でそれをコアに分けて回す (`tests/common/par.rs`) ので、`cargo test` でも
遅くはない。

テストには次のものが入っている。

- lexer などの単体テスト
- check の診断 (エラーの後も止まらないこと、キャッシュされた呼び出し元越しに総称関数の本体のエラーを拾うこと)
- キャッシュとインクリメンタルビルド
- panic の場所、複数クレート
- サイズ予算 (`tests/budget.rs`)
- zust-ide のゴールデン
- zust-analyzer の結合テスト (実際のバイナリを stdio で動かす)

LSP の側だけを直したときは、次のコマンドで足りる。

```bash
cargo test --release -p zust-ide -p zust-analyzer
```

### rustc と結果を比べる

```bash
tests/compare.sh
```

`examples/*.rs` を zustc と rustc の両方でビルドし、出力と終了コードが一致するかを確かめる。キャッシュなしのビルドと、
キャッシュからの再ビルドの両方を比べる。valgrind があれば、メモリエラーと確実なリークも失敗にする。

```bash
tests/probe.sh
```

よく使う機能の小さいプログラム (`tests/probe/*.rs`) を rustc と比べ、通った数を出す。言語の機能を足したら、ここに
probe を足す。

### セルフホスト

```bash
tests/selfhost.sh
```

zustc 自身のソースの一部 (lexer / prelude / link / macros / parse / 型推論 など) を zustc でコンパイルし、rustc で
コンパイルしたものと出力が同じかを確かめる。

```bash
tests/selfhost_compiler.sh
```

コンパイラ全体を zustc と rustc の両方でコンパイルする。できた 2 つのコンパイラが同じ `.o` を出すかを確かめる。zustc 版で
自分自身をもう一度コンパイルし、同じものになるか (不動点) も見る。

### rustc 自身のテスト集

`bench/rust_ui.py` は、rustc の UI テスト (rust-lang/rust の `tests/ui`。rustc 1.100.0 のコミットに固定) を zustc で
走らせる。最初に一度取ってくる (`tests/ui` と `tests/auxiliary` だけの部分 clone、約 170 MB を `out/rust-tests` に):

```bash
python3 bench/rust_ui.py --fetch
```

```bash
python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --check
```

テストの種類は、compiletest と同じく頭の指示から決める。run-pass のテストは、ビルドでき、終了コード 0 で、
`.run.stdout` と同じものを出すこと。check-pass のテストは `zustc check` が通すこと。compile-fail のテストは、
`//~ ERROR` の注釈が指す行にエラーを出して拒むこと。zustc に求められないもの (nightly の `#![feature]`・補助の
クレート・`--test` と `--cfg` 以外の `compile-flags`・`revisions`・ほかのターゲット) が要るテストは「対象外」に数える。

`bench/rust_ui_passing.txt` は zustc が通るテストの一覧。そのどれかが通らなくなると CI が失敗する。変更で通る
テストが増えたら、一覧を書き直す:

```bash
python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --write-baseline
```

`bench/rust_ui_ratchet.txt` は通過数の ratchet で、zustc が通すべきテストの種類ごとの最低数。これを下回ると CI が
失敗する。`--write-baseline` はこれをその実行の数まで上げるが、下げはしない。`bench/rust_ui_accidental.txt` の
通過は数えない。

`--filter 文字列` でパスが合うものだけ、`--kinds run-pass` で 1 つの種類だけを走らせる。すべてのテストの結果は
`out/rust_ui_results.txt` にある。

### ゴールデン

`crates/ide/tests/golden/*.txt` は zust-ide の答えの記録。意図して答えを変えたら、次のコマンドで書き直す。

```bash
UPDATE_GOLDEN=1 cargo test -p zust-ide --test golden
```

書き直す前に、差分が改善かどうかを確かめる。rust-analyzer と同じ答えになったかは、次のコマンドで見られる。

```bash
python3 bench/ide_golden_ra.py --detail
```

## KPI と残っている作業

コンパイラか analyzer に触れる変更は、残す前に必ず測る。

- `python3 bench/kpi_track.py "短い説明"` は、cachegrind で次の命令数を数える:
  - 5000 関数の生成プログラムの no-op ビルド・1 関数の編集・`check`
  - trait と型推論を多く使うプログラム (`bench/gen_traits.py`)
  - cratebench のクレート
  - アプリケーションのコードの形 (`bench/gen_patterns.py mix`): インタプリタ、レコードの処理、`?` によるエラー処理、
    状態機械、trait object のプラグイン、ライフタイムを使うコピーなしの構文解析、`macro_rules!`、入れ子のモジュール、
    総称のコンテナ
  - `--lsp` を付けると、zust-analyzer のセッション

  `python3 bench/patterns.py` は、それぞれのパターンを 1 つずつ測る (コールドビルド・`check`・編集後のビルド。
  `--instructions` を付けると命令数も)。zustc でビルドしたものが rustc でビルドしたものと同じ出力かも確かめる。
  新しいパターンは `bench/gen_patterns.py` に足す。`tests/bench_patterns.rs` がすべてのパターンをビルドする。

  `bench/kpi_track.py` は `bench/kpi_log.tsv` / `bench/kpi_log.md` に 1 行足す (`bench/patterns.py` は表を出すだけ)。
  直近 7 行のノイズの幅を外れた値は `REGRESSION` と出る。変更を残す前に原因を突き止めるか、コストをコミットメッセージに
  書く。
  pull request では `--dry-run` で測り、ログに行をコミットしない。`KPI` ワークフローが pull request をベースと比べて測り、
  結果をその pull request のコメントに残す (下の CI を参照)。行は main で作るコミットで足す。ブランチで足した行は、その間に
  main が足す行と毎回ぶつかる。
  `bench/kpi_ratchet.txt` は KPI ごとの上限を持つ。head がどれかの上限を許容幅より超えた pull request は、`KPI`
  ワークフローが失敗にする。コストを取り除くか、その pull request で上限を上げ (KPI のコメントがその行を出す)、なぜその
  コストに見合うかを説明に書く。main への push ごとに、main が下回った上限をその値まで下げる。上限を上げるのは、この編集だけ。
- `bench/rust_ui.py --check` は 0 regressions でなければならない。テストが通るようになったら、ベースラインを書き直す
  (`--write-baseline`)。
  - 新しく通った compile-fail のテストが何で拒否されたかを見る。注釈の行で拒否されても、そのテストが調べている理由と
    違うなら `bench/rust_ui_accidental.txt` に入れる (ベースラインに入らなくなる)。
- 実在のクレート: `python3 cratebench/run.py` は、それぞれのクレートとアプリを zustc と rustc でビルドし、出力を比べる。
- 大きなブロッカー (ライフタイム、trait の検査、unwinding、メソッドを経由する型推論、未対応の型、prelude の大きさ、
  wasm の出力、スレッドと async) は [issue](https://github.com/mizchi/zust/issues) にある。それぞれ、どこから始めるか、
  どう測るか、何を悪化させてはいけないかを書いてある。

## ベンチマークと回帰監視

```bash
bench/regress.sh save
```

機能を足す前に、今の状態をベースライン `main` として保存する。

```bash
bench/regress.sh
```

ベースラインと比べる。次のどれかがあれば失敗する。

- criterion (`benches/compile.rs`) が「回帰」と判定したもの
- サイズ予算 (`bench/budget.txt`) の超過
- rustc との出力の差 (`tests/compare.sh`)

### 判定の決まり

- 5% 未満の変化は無視する。有意水準は 1%。cold 系は ±10% ぶれるので、閾値を 15% にしている。
- 値はマシンの状態で変わる。比べるときは同じ時間帯に、ベースラインのコミットと並べて測る (git worktree で HEAD を同時に
  ベンチすると確か)。原因を詰めるには callgrind の命令数が安定している。
- サイズ予算は、例ごとに、バイナリ・`.o`・キャッシュの上限のバイト数を持つ。
  - 意図して増やすなら `UPDATE_BUDGET=1`。
  - 新しい例を足しただけなら `UPDATE_BUDGET=new` (既存の上限は変えない)。

  予算を緩める前に、増えた理由 (付く prelude が増えた、など) を詰める。これまでの例は
  [docs/ja/reports/2026-11-15-17-regression-catches.md](docs/ja/reports/2026-11-15-17-regression-catches.md) にある。

### ほかのベンチマーク

| スクリプト | 何を測るか |
|---|---|
| `python3 bench/vs_rustc.py [NAME..]` | rustc との比較 (cold・no-op・1 か所編集の時間、ディスク、バイナリ、実行時間) |
| `python3 bench/runtime.py` | 実行時間とバイナリだけ (軽い版) |
| `python3 bench/run.py 1000 5000` | N 関数の生成したプログラムでの cold / no-op / 1 関数編集 |
| `python3 bench/ws_bench.py --rustc` | 複数クレートのワークスペース |
| `python3 bench/deps_bench.py` | 依存の多い複数クレート |
| `python3 bench/lsp_compare.py --cold` | LSP サーバの速さ (rust-analyzer / rust-glancer と) |
| `python3 bench/lsp_parity.py` | LSP の答えを rust-analyzer と比べる |

`lsp_parity.py` は、前回の数字と比べて差を出せる。

```bash
python3 bench/lsp_parity.py --compare bench/lsp_parity.json --json bench/lsp_parity.json --out bench/lsp_parity_result.md
```

`--detail` を付けると、違った所を出す。

### ミューテーションテスト

```bash
bench/mutants.sh diff
```

origin/main からの差分の行だけを変異させ、テストで見つかるかを見る。`bench/mutants.sh pkg zust-ide` (クレート) や
`bench/mutants.sh file crates/ide/src/symbols.rs` (ファイル) でも絞れる。見つからなかった変異は
`mutants.out/missed.txt` に出る。テストを足して潰す。

## CI とベンチマークのダッシュボード

- `.github/workflows/ci.yml`: push と pull request ごとに、release ビルド、テスト
  (`cargo nextest run --release --workspace --profile ci` と doc テスト)、`tests/compare.sh`、tests/ui の一覧との比較を
  走らせる。test ジョブはクレート内の ThinLTO なしでビルドし (`CARGO_PROFILE_RELEASE_LTO=off`: ワークスペースのビルドが
  2/3 ほどの時間になる)、ビルドの間に、キャッシュしたパッケージから valgrind を入れる。サイズ予算はリンカで変わるので、CI は wild 0.11.0 を入れて `ZUSTC_BENCH_LINKER=wild` で測る
  (`bench/budget.txt` も wild の大きさ)。
- `.github/workflows/kpi.yml`: pull request への push ごとに走る。head と分岐点 (merge base) を
  `bench/kpi_track.py --dry-run --only <セクション>` で測る (zust-analyzer のセッションも含む)。測定は 3 つに分けて、並んだ
  ランナーで同時に走らせる (ベースの分も head の分と同時に): gen5000、traits・crates・patterns、LSP セッション。
  ベースの値は、測った環境 (ツールチェーン・valgrind・OS・C ライブラリ) をキーにして保存する。どの部分も、自分の環境が
  そのキーの環境と同じかを確かめる。pull request に書き込めるのは、コメントするジョブだけ。命令数は、同じイメージならどのランナーでも
  同じになる。ベースの値はキャッシュするので、一度測ったベースは測り直さない。`bench/kpi_pr.py` が
  pull request のコメントを 1 件だけ保ち、更新する。そこには KPI ごとのベースとの差と、push ごとの行 (前の push との差つき) が載る。1% を超えて増えた KPI には印を付け、
  警告を出す。head が `bench/kpi_ratchet.txt` の上限を許容幅より超えると、ジョブは失敗する (`bench/kpi_ratchet.py`)。
  main への push ごとに、`ratchet` ジョブが main の下回った上限をその値まで下げ、CI を飛ばすコミットにする。上限は
  測った環境でだけ使う。環境が変わったら何も確かめず、次の main への push で測り直す。
- `.github/workflows/bench.yml`: main への push ごとに `bench/ci_bench.py` を走らせ、
  [github-action-benchmark](https://github.com/benchmark-action/github-action-benchmark) で gh-pages ブランチの `dev/bench/` に
  結果を積む。ダッシュボードは https://mizchi.github.io/zust/dev/bench/ (リポジトリの Settings → Pages で gh-pages ブランチを
  公開しておく)。
  - zustc: 1000 / 5000 関数のプログラムのビルド (cold / no-op / 1 関数編集)・check の時間と命令数、ピーク RSS、出力と
    キャッシュの大きさ、`bench/rt` の実行時間
  - zust-analyzer: `bench/lsp_parity.py` (zustc 自身のソース) の各問い合わせの p50、起動から索引ができるまで、RSS
  - rust-analyzer と同じ答えを返した割合
  - rustc の tests/ui の通った割合
  - 前回より 30% 以上悪くなった (一致の割合は 5%) ものは、ワークフローの結果に警告が出る (失敗にはしない)

ローカルで同じものを測るには:

```bash
python3 bench/ci_bench.py --out-dir out/ci
```

## 英語の文書と日本語訳

`README.md` (概要、zustc と zust-analyzer の使い方、測った数値の要約)、`SUPPORT.md` (分野ごとのサポート範囲)、
`PERFORMANCE.md` (KPI と、ほかのツールとの比較) は英語だけ。ロードマップはこの文書にある。この 3 つのファイルの測った数値
(`<!-- auto:.. -->` の間と、`<!-- ui:.. -->` / `<!-- cf:.. -->` の直後) は CI が書き直す
(bench.yml が main への push ごとに `scripts/update_readme.py` を実行する) ので、手で直さない。英語の文書と日本語訳の組は
`CONTRIBUTING.md` / `CONTRIBUTING.ja.md`、
`docs/X.md` / `docs/ja/X.md` (レポートも)。片方を変えたら、もう片方も変える。
[jev-lint](https://github.com/mizchi/jev-lint) の `docs` が、それぞれの組が同じことを言っているかをモデルに聞く。

- `scripts/docs_manifest.py` は、組から jev-lint のマニフェスト (`.jev-lint/documents*.json`) を書く。`--check` は、
  相手のないファイルがあるか、マニフェストが古ければ失敗する。
- `scripts/docs_check.sh` は、今の文書についての記録した判定を台帳 (`.jev-lint/judgments/`) から読む。台帳は jev-lint の
  judgment ledger で、追記だけのファイルからなり、ブランチ同士はその和集合としてマージできる。答えは比べる本文・問い・
  モデルで引く。オフラインで、API キーは要らない (CI が走らせる)。組が食い違っていれば失敗する。判定を記録した後に
  文書が変わった組は台帳に答えがなく、API キーがなければ判定できないので、`TYPESAFE_API_KEY` がないときはその
  マニフェストを警告を出して飛ばす (`DOCS_CHECK_STRICT=1` なら飛ばさず失敗にする。終了コード 3)。組がそろっているかと
  マニフェストが最新かは、どちらでも確かめる。
- `scripts/docs_check.sh --update` は、台帳に答えのない組だけをモデルに判定させ、答えを台帳に足す。
  https://typesafe.ai の `TYPESAFE_API_KEY` (か `TYPESAFEAI_API_KEY`) が要る。文書の本文はその API に送られる。64 組を
  全部判定して約 $0.03。

コミットフックは、clone ごとに一度有効にする。

```bash
git config core.hooksPath .githooks
```

Markdown を変えたコミットでは、フックが `scripts/docs_check.sh --update` を走らせ、新しい答えをコミットに足し、組が
食い違っていればコミットを止める。判定はモデルの判断なので、出た組を読んで間違っている側を直す (判定の方が間違っていれば
`--no-verify` でコミットする)。

## デバッグ用の環境変数

| 変数 | 意味 |
|---|---|
| `ZUSTC_IR_VERIFY=1` | 組み立て直した命令の値の型が、型検査の記録と同じかを確かめる |
| `ZUSTC_CLIF_VERIFY=1` / `ZUSTC_CLIF_DUMP=1` | Cranelift の IR を検証する / 出す |
| `ZUSTC_PREP_VERIFY=1` | 前処理の差分の結果を、全部やり直した結果と比べる |
| `ZUSTC_CHECK_FORK_MIN=N` | `check` が関数の wave を子プロセスに分けるのは、子 1 つあたり N 個以上あるときだけ (既定 32)。`1` にするとほぼすべての wave で fork し、診断が `-j 1` と同じかを試せる |
| `ZUSTC_BUILD_PAR_MIN=N` | ビルドが wave の関数を fork した子でコンパイルするのは、子 1 つあたり N 個以上あるときだけ (既定 16)。`1` にするとほぼすべての wave を分け、.o が `-j 1` と同じかを試せる |
| `ZUSTC_NO_POLONIUS=1` | Polonius Alpha と違うところで borrow check を NLL の規則にする (#1): 返される reborrow は関数の終わりまで生き、ローカルの一部への代入は借用を kill しない |
| `ZUSTC_DUMP_PRE=FILE` | 前処理 (macro_rules! の展開・`mod x;` の結合) の後のソースを FILE に書く |
| `ZUSTC_LOG_COMPILED=1` | コンパイルした関数の名前を出す (キャッシュから読めなかったものはその理由も) |
| `ZUSTC_PROF=1` | 段ごとの時間を細かく出す |
| `ZUSTC_NO_STRIP=1` | 出力のシンボルを残す (callgrind などで関数名を見る) |
| `ZUSTC_BENCH_LINKER` | criterion のベンチのリンカを固定する |

ほかの `ZUSTC_NO_*` は、最適化やインクリメンタルの仕掛けを 1 つずつ切って比べるためのもの。名前でコードを検索すると
意味が分かる。

## 変更の進め方

- 言語の機能を足すときは、まず `examples/` か `tests/probe/` に rustc と比べるプログラムを足す。
- 変更の後は `cargo test --release --workspace` を通す。コンパイラを変えたら `tests/compare.sh` も通す。
  速さやサイズに効く変更なら `bench/regress.sh` も通す。
- LSP の答えを変えたら、`bench/lsp_parity.py` で一致の割合と時間が下がっていないことを見る。
- まとまった作業をしたら、レポートを書く (次の節)。
- rust-analyzer との違いや、残したことは下の [ロードマップ](#ロードマップ) (か issue) に、サポート範囲は
  [SUPPORT.md](SUPPORT.md) に書く (英語。割合のある新しい分野には tests/ui のディレクトリを
  書いた `<!-- ui:DIR -->` を付ける)。

### レポートの書き方

作業の記録は `docs/reports/YYYY-MM-DD-NN-<slug>.md` に英語で置く。日本語版を書くなら、同じ名前で `docs/ja/reports/` に置く。

- `YYYY-MM-DD` は作業した日。
- `NN` はその日の中の順番 (01 から)。
- `<slug>` は内容を表す短い kebab-case。

ファイル名で並べると日付順になる。中身は次の形にする。

```markdown
# Title

Date: YYYY-MM-DD

What was done, why, how it was measured, the results (numbers), what was learned, what is left
```

数字は測った時点のものとして残す (後から書き換えない)。書いたら、[docs/reports/README.md](docs/reports/README.md) の
一覧に 1 行足す (日本語版を書いたら [docs/ja/reports/README.md](docs/ja/reports/README.md) にも)。

### コミット

- 1 つのコミットは 1 つのまとまった変更にする。
- メッセージは、どの部分か (`ide:` / `analyzer:` / `docs:` など) と、何を変えたかを書く。
- 名前は zustc / zust-analyzer / zust-* に揃える。

## ロードマップ

### コンパイラ (zustc)

- **セキュリティ** ([TODO.md](TODO.md)。[レポート](docs/reports/2026-11-21-01-security-cves.md)): セキュリティのテストが
  見つけて残した穴 (`tests/security/` の `//@ todo` のケース) と、wasm32 のターゲットの残り
- **rustc の tests/ui** (`python3 bench/rust_ui.py`。[レポート](docs/reports/2026-11-18-07-rustc-ui-tests.md))
  - 通る割合を上げる ([README の Status](README.md#status)。分野ごとは [SUPPORT.md](SUPPORT.md))
  - 大きな穴: unwinding (`catch_unwind`、`panic::set_hook`)、FFI の構造体の値渡し、型推論 (期待型からしか分からない
    型パラメータ)、本体の中の式の位置の `type` 別名とマクロ、const の中のクロージャ、ほかの型の中の引数の型の
    `impl Trait` (`&Vec<impl Display>`)
  - compile-fail のテストに要る検査: 既定値と境界の well-formedness、coherence、ブランケット impl の境界、dyn / async を
    通る Send、ライフタイムの shadowing、lint
- **借用検査** ([レポート](docs/reports/2026-11-18-08-borrow-checking.md))
  - シグネチャに対する region のエラー (E0621、E0521、"lifetime may not live long enough"): universal region と、本体の
    宣言に対する outlives の検査
  - 型を通って流れる借用: ローカル変数は読んだもの全部の借用を持つので、ブロックの外に運ばれる値の E0515 と E0597 は
    直接借りているものしか見ていない
  - E0716 (借りている間に drop される一時値)、E0510、スコープの終わりの `Drop` による使用、配列と構造体のパターンからの
    部分的なムーブ
  - 1 つの関数で 2 つ以上のエラー
  - コスト: `zustc check` の命令数が +2.2%。ほとんどは lowering の間の記録
- **言語**
  - モジュール、`DoubleEndedIterator` (値としての `rev`)、`HashSet` / `BTreeMap`
  - 単相化したインスタンスをレイアウトで共有する (`Vec<A>` と `Vec<B>` を 1 つにする。blaze の polymorph)
  - 自前の最小の std (relax のやり方)
- **インクリメンタル**
  - alpha 正規化したハッシュ
  - 依存するクレートの解析結果 (アイテムの要約、Program の一部) を、依存する側で使い回す (今はソースを解析し直す)
  - 変わった `.o` だけを差し替えるリンク
  - libwild を埋め込み、`.o` をメモリに持つ (`.o` を書かず、`exec` もしない)
- **インクリメンタルなリンク**
  - `.text` に余白を残し、変わった関数だけを差し替える
  - ジャンプテーブルを通した呼び出し (blaze のやり方)
- **コールドビルド**
  - アイテムの要約と構文解析を並列にする
  - コールドビルドで AST を 2 回解析するのをやめる
- **サイズ**
  - コンパイラの中の ICF (コード + 再配置先のハッシュ)
  - 文字列のセクションをまとめる
- **`-O`**
  - 自前のインライン化とスタックスロットの転送 (Cranelift に SROA がないのを補う)

### zust-analyzer: rust-analyzer との残った違い

`bench/lsp_parity.py`・`bench/ide_golden_ra.py` で比べ、参照を 1 つずつ確かめた後に残ったもの
([docs/reports/2026-11-18-06-remaining-differences.md](docs/reports/2026-11-18-06-remaining-differences.md))。

#### 型推論

- [ ] 依存するクレートの中でマクロが `Iterator` の impl を作るイテレータの `Item` (serde_json の `map::Iter`。
  `delegate_iterator!` から)。zust-analyzer は、今はそこで間違った型ではなく何も答えない
- [ ] 依存するクレートから返る、型引数のない `Iter` 型は、同じ名前の型とモジュールだけで見分けている。ジェネリックな型は
  まだ名前と引数の数で合わせている

#### 定義と参照

- [ ] `x.to_string()` の定義へのジャンプ: rust-analyzer はその型の `Display::fmt` の impl に飛ぶ。zust-analyzer は
  `ToString::to_string` に行く
- [ ] `ToString::to_string` の参照で、rust-analyzer は trait の宣言 (string.rs:2956) ではなくブランケット impl の
  メソッド (string.rs:2969) を挙げる
- [ ] std のモジュール (`std::collections`、`std::sync`) の参照: rust-analyzer は std 自身のソースの中のパス
  (`collections/mod.rs` の `self::hash_map`) も数える。zust-analyzer はライブラリのテキストを持たないので、ワーク
  スペースだけを数える (fixture: 参照 627/629)
- [ ] rust-analyzer は `crates/syntax/build.rs` の `-> Key` (ビルドスクリプト自身の `type Key = u128`) を
  `zust_base::key::Key` の参照に数える。`lexer.rs` を `#[path]` でビルドスクリプトと共有していることから来る
  rust-analyzer の癖に見える。zust-analyzer は数えない
- [ ] マクロが作るアイテムの定義 (str のパターンのイテレータの `next`、`u32::MAX`): rust-analyzer はマクロの呼び出し
  (`generate_pattern_iterators! {`、`uint_impl! {`) も挙げる。zust-analyzer はマクロの本体の中のアイテムだけを挙げる

#### rename

- [ ] `Default::default` の rename (rust-analyzer は依存するクレートの中も書き換える。こちらはワークスペースだけ。
  意図した違い)
- [ ] `use a::Point as Pt` の別名の rename (意図した違い)

#### 速さとメモリ

- [ ] zustc を対象にした参照の p90 が 32.8 ms (ファイルを複数のスレッドで走査する前は 65 ms)。遅いのは、どこにでも
  出てくる名前のメソッド (`len`、`iter`) を、出てくる所ごとに解決する場合
- [ ] 依存するクレートの索引でメモリが 55 MB から 66 MB に増えた。使わない定義を捨てる / 遅延して読む

## ライセンス

送られた貢献は、明示しない限り、README の「ライセンス」のとおり MIT OR Apache-2.0 で提供されたものとする。他のプロジェクトの
コードを取り込むときは、ライセンスが合うものだけにし、上流のリポジトリを git submodule にする (`cratebench/vendor/` と同じ) か、元のライセンスのファイルも一緒に置く。
