# 開発のしかた

[English](CONTRIBUTING.md) | 日本語

zust (zustc と zust-analyzer) を直す・足すときの手順。使い方は [README.ja.md](README.ja.md)、中のしくみは
[docs/ja/architecture.md](docs/ja/architecture.md)。

## 準備

README の「必要なもの」に加えて、次のものを使う。

- rustc: tests/compare.sh や tests/probe.sh が、同じプログラムを rustc でもビルドして結果を比べる。
- python3: ベンチマークのスクリプト (`bench/*.py`)。
- valgrind (あれば): tests/compare.sh がメモリエラーとリークも見る。
- rust-analyzer (あれば): `bench/lsp_parity.py` と `bench/ide_golden_ra.py` が答えを比べる。
- cargo-mutants (あれば): ミューテーションテスト。
- wild (あれば): リンクが速くなる。ベンチマークの数字は wild で測っている。

```bash
cargo build --release --workspace
```

`cratebench/run.py` と `bench/deps_bench.py` は、実在のクレートのソースを git submodule から使う。最初に一度取ってくる
(`git clone --recurse-submodules` で clone してもよい)。

```bash
git submodule update --init cratebench/vendor
```

`target/release/zustc` と `target/release/zust-analyzer` ができる。テストやスクリプトの多くは、この 2 つの release
ビルドを使う。

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

全部のテスト。次のものが入っている。

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

### ゴールデン

`crates/ide/tests/golden/*.txt` は zust-ide の答えの記録。意図して答えを変えたら、次のコマンドで書き直す。

```bash
UPDATE_GOLDEN=1 cargo test -p zust-ide --test golden
```

書き直す前に、差分が改善かどうかを確かめる。rust-analyzer と同じ答えになったかは、次のコマンドで見られる。

```bash
python3 bench/ide_golden_ra.py --detail
```

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

- `.github/workflows/ci.yml`: push と pull request ごとに、release ビルド、`cargo test --release --workspace`、
  `tests/compare.sh` を走らせる。サイズ予算はリンカで変わるので、CI は wild 0.11.0 を入れて `ZUSTC_BENCH_LINKER=wild` で測る
  (`bench/budget.txt` も wild の大きさ)。
- `.github/workflows/bench.yml`: main への push ごとに `bench/ci_bench.py` を走らせ、
  [github-action-benchmark](https://github.com/benchmark-action/github-action-benchmark) で gh-pages ブランチの `dev/bench/` に
  結果を積む。ダッシュボードは https://mizchi.github.io/zust/dev/bench/ (リポジトリの Settings → Pages で gh-pages ブランチを
  公開しておく)。
  - zustc: 1000 / 5000 関数のプログラムのビルド (cold / no-op / 1 関数編集)・check の時間と命令数、ピーク RSS、出力と
    キャッシュの大きさ、`bench/rt` の実行時間
  - zust-analyzer: `bench/lsp_parity.py` (zustc 自身のソース) の各問い合わせの p50、起動から索引ができるまで、RSS
  - rust-analyzer と同じ答えを返した割合
  - 前回より 30% 以上悪くなった (一致の割合は 5%) ものは、ワークフローの結果に警告が出る (失敗にはしない)

ローカルで同じものを測るには:

```bash
python3 bench/ci_bench.py --out-dir out/ci
```

## デバッグ用の環境変数

| 変数 | 意味 |
|---|---|
| `ZUSTC_IR_VERIFY=1` | 組み立て直した命令の値の型が、型検査の記録と同じかを確かめる |
| `ZUSTC_CLIF_VERIFY=1` / `ZUSTC_CLIF_DUMP=1` | Cranelift の IR を検証する / 出す |
| `ZUSTC_PREP_VERIFY=1` | 前処理の差分の結果を、全部やり直した結果と比べる |
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
- rust-analyzer との違いや、残したことは [TODO.md](TODO.md) に書く (英語)。

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

## ライセンス

送られた貢献は、明示しない限り、README の「ライセンス」のとおり MIT OR Apache-2.0 で提供されたものとする。他のプロジェクトの
コードを取り込むときは、ライセンスが合うものだけにし、上流のリポジトリを git submodule にする (`cratebench/vendor/` と同じ) か、元のライセンスのファイルも一緒に置く。
