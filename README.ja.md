# zust

[![CI](https://github.com/mizchi/zust/actions/workflows/ci.yml/badge.svg)](https://github.com/mizchi/zust/actions/workflows/ci.yml)
[![Benchmark](https://github.com/mizchi/zust/actions/workflows/bench.yml/badge.svg)](https://mizchi.github.io/zust/dev/bench/)

[English](README.md) | 日本語

Rust のサブセットを速くインクリメンタルにビルドするコンパイラ **zustc** と、軽い LSP サーバ **zust-analyzer**。

- **zustc**: 自前のフロントエンド (lexer / parser / 型推論) と Cranelift (既定) か自前の x86-64 バックエンド minclif で
  コード生成する。開発中の「1 か所直してビルド」を速く、小さくすることに特化している。
  - 再ビルドの時間
  - ピークメモリ
  - キャッシュのディスク消費
  - 出力バイナリのサイズ

  zustc 自身を zustc でコンパイルできる。
- **zust-analyzer**: エディタ向けの LSP サーバ。定義・参照・hover・補完・rename などを、字句の列と定義の表からの軽い解析で
  返す。普通の Rust のワークスペース (std と依存クレートも索引する) で使える。

> **状態**: 実験的なプロジェクト。zustc は Linux x86-64 だけで、Rust の全部は扱えない ([対応している言語](docs/ja/language.md))。
> 検査 (型、region の誤りを除く借用検査) は一部だけなので、rustc の代わりに正しさを確かめる道具ではない。zust-analyzer は rust-analyzer の一部の機能だけ。

## どのくらい速いか

### rustc との比較

`python3 bench/vs_rustc.py` と `python3 cratebench/run.py` で測った (2026-11-18、4 コア、rustc 1.100.0。時間は 3 回の
中央値)。作業の回ごとに測り直す。gen1000 は 1000 関数の生成したプログラム:

| gen1000 | cold | no-op | 1 関数を編集 | ディスク | バイナリ (strip 後) | 実行 |
|---|---:|---:|---:|---:|---:|---:|
| zustc | 121 ms | 16 ms | 23 ms | 766 KiB | 324 KiB | 9 ms |
| zustc `-O` | 285 ms | 56 ms | 56 ms | 1199 KiB | 341 KiB | 3 ms |
| rustc `-C incremental` | 592 ms | 169 ms | 331 ms | 10.8 MiB | 1038 KiB | 6 ms |
| rustc `-O` | 14902 ms | - | - | - | 377 KiB | 2 ms |
| **zustc の rustc `-C incremental` 比** | **5.2 倍速い** | **11 倍速い** | **16 倍速い** | **1/14** | **1/3.2** | |

小さなプログラム (`bench/rt`: fib、hashmap、matmul、nbody、sieve、sort、strings) と実際のクレート (cratebench: fnv、
adler2、hex、itoa、rustc-hash、cfg-if。6 つとも rustc と同じ出力を出す):

| | zustc の rustc `-C incremental` 比 (bench/rt) | zustc の rustc 比 (cratebench) |
|---|---:|---:|
| cold ビルド | 2.9〜4.1 倍速い (24〜49 ms と 82〜178 ms) | 3.0〜4.0 倍速い (25〜62 ms と 93〜236 ms) |
| 編集して再ビルド | 12〜18 倍速い (5〜7 ms と 68〜98 ms) | 5.1〜7.2 倍速い (12〜23 ms と 68〜122 ms) |
| バイナリ | 1/11〜1/24 | 1/90〜1/550 (8〜48 KiB と 4.4 MiB) |
| rustc `-O` に対する実行時間 | zustc は 1.1〜4.1 倍遅い、zustc `-O` は 1.1〜3.2 倍 | zustc は 3.1〜17 倍遅い、zustc `-O` は 0.9〜7.3 倍 |

同じ gen1000 の比較の推移 (rustc 自身の時間は回ごとに ±10% ほど、zustc の cold ビルドは ±20% ほど動く。前のコミットと交互に走らせると、
1501b30 と 8e797a9 の cold の中央値は 96 ms と 98 ms で同じ):

| 測った時点 | zustc の cold | rustc `-C incremental` の cold | 比 | zustc の編集 | rustc の編集 | 比 |
|---|---:|---:|---:|---:|---:|---:|
| 2026-11-15 ([報告](docs/ja/reports/2026-11-15-15-status.md)) | 118 ms | 827 ms | 7.0 倍 | 28 ms | 417 ms | 14.9 倍 |
| 2026-11-18、72201ab | 131 ms | 643 ms | 4.9 倍 | 22 ms | 348 ms | 15.8 倍 |
| 2026-11-18、ブロックの中のアイテム (cbcd294) | 117 ms | 620 ms | 5.3 倍 | 22 ms | 334 ms | 15.2 倍 |
| 2026-11-18、推論と呼び出しの引数の借用検査 (8e797a9) | 94 ms | 602 ms | 6.4 倍 | 22 ms | 353 ms | 16.0 倍 |
| 2026-11-18、組み込みマクロ (1501b30) | 127 ms | 600 ms | 4.7 倍 | 24 ms | 316 ms | 13.2 倍 |
| 2026-11-18、トレイトの `impl Trait`、マクロの追加 (8cb2f88) | 131 ms | 605 ms | 4.6 倍 | 20 ms | 357 ms | 17.9 倍 |
| 2026-11-18、型推論の `assert!` (4d210d2) | 126 ms | 619 ms | 4.9 倍 | 23 ms | 328 ms | 14.3 倍 |
| 2026-11-18、型推論の `assert_eq!` (441d0c2) | 119 ms | 605 ms | 5.1 倍 | 25 ms | 338 ms | 13.5 倍 |
| 2026-11-18、レジスタの値の借用 (fb2f2f2) | 112 ms | 595 ms | 5.3 倍 | 22 ms | 367 ms | 16.7 倍 |
| 2026-11-18、`try_from`、`::std` のパス、std の関数の追加 (096a6bc) | 102 ms | 624 ms | 6.1 倍 | 22 ms | 327 ms | 14.9 倍 |
| 2026-11-18、ビルドごとの表を作らない prelude の選別 (f077e7c) | 89 ms | 623 ms | 7.0 倍 | 22 ms | 320 ms | 14.5 倍 |
| 2026-11-18、`dyn Trait<A>` (f606b34) | 89 ms | 641 ms | 7.2 倍 | 22 ms | 309 ms | 14.0 倍 |
| 2026-11-18、境界に合わせた整数リテラル、`Box` の比較 (60cbe60) | 112 ms | 655 ms | 5.8 倍 | 21 ms | 341 ms | 16.2 倍 |
| 2026-11-18、`()` を返す `impl Trait` (e7fdc21) | 121 ms | 592 ms | 4.9 倍 | 23 ms | 331 ms | 14.4 倍 |
| 2026-11-18、ユーザーのコードが定義しなおせる std の型名 | 120 ms | 625 ms | 5.2 倍 | 21 ms | 331 ms | 15.8 倍 |

zustc のコードの実行速度は rustc の debug ビルドに近く、7 つの小さなプログラムのすべてでそれより速い (そこでは rustc の debug
ビルドは rustc `-O` の 3〜26 倍遅い)。

zust-analyzer と rust-analyzer を、zustc 自身のソース (40 か所の問い合わせ) で比べた (`python3 bench/lsp_parity.py`、
2026-11-18):

| | 起動から名前が引けるまで | メモリ | references の p50 | 一致 (40 か所) |
|---|---:|---:|---:|---:|
| zust-analyzer | 144 ms | 70 MB | 4.6 ms | 95.3% |
| rust-analyzer | 6302 ms | 2150 MB | 154.1 ms | 96.2% |

詳しくは [bench/lsp_parity_result.md](bench/lsp_parity_result.md)。

### KPI の推移

KPI はビルドの速度、ビルドキャッシュのサイズ、LSP の応答時間で、1000 / 5000 関数の生成したプログラムに対して
`python3 bench/ci_bench.py` で測る。ビルドは命令数 (valgrind) で比べる。共有のマシンでは実時間が ±20% ほど揺れるため。

| 時点 | gen5000 no-op ビルド | gen5000 1 関数編集 | gen5000 `check` | gen5000 キャッシュ | gen1000 キャッシュ | gen5000 cold の最大 RSS |
|---|---:|---:|---:|---:|---:|---:|
| 借用検査の前 (ac2361e) | | | 3883.9 M | | | |
| 借用検査を入れた後 ([報告 08](docs/ja/reports/2026-11-18-08-borrow-checking.md)) | 746.0 M | 709.3 M | 3971.0 M | 1902.8 KiB | 387.2 KiB | 76.0 MiB |
| 互換性の作業 ([報告 09](docs/ja/reports/2026-11-18-09-compat-with-kpis.md)、03367bc) | 685.7 M | 648.1 M | 3912.7 M | 1902.8 KiB | 387.2 KiB | 54.6 MiB |
| prelude の選別の高速化 (3a1fd22) | 661.1 M | 622.4 M | 3887.3 M | 1902.8 KiB | 387.2 KiB | 51.8 MiB |
| async、static、const fn (e598fdf) | 661.9 M | 622.6 M | 3892.9 M | 1902.8 KiB | 387.2 KiB | 51.9 MiB |
| ブロックの中のアイテム (cbcd294) | 661.5 M | 622.9 M | 3893.1 M | 1902.8 KiB | 387.2 KiB | 51.6 MiB |
| 推論と呼び出しの引数の借用検査 (8e797a9) | 661.6 M | 623.0 M | 3893.3 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| 組み込みマクロ (1501b30) | 661.3 M | 622.8 M | 3893.3 M | 1902.8 KiB | 387.2 KiB | 51.6 MiB |
| トレイトの `impl Trait`、マクロの追加 (8cb2f88) | 661.4 M | 622.9 M | 3893.3 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| 型推論の `assert!` (4d210d2) | 662.1 M | 622.9 M | 3893.6 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| 型推論の `assert_eq!` (441d0c2) | 661.4 M | 622.8 M | 3893.5 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| レジスタの値の借用 (fb2f2f2) | 662.0 M | 622.7 M | 3893.5 M | 1902.8 KiB | 387.2 KiB | 51.6 MiB |
| `try_from`、`::std` のパス、std の関数の追加 (096a6bc) | 662.9 M | 623.7 M | 3892.3 M | 1902.8 KiB | 387.2 KiB | 52.0 MiB |
| ビルドごとの表を作らない prelude の選別 (f077e7c) | 663.8 M | 620.8 M | 3892.9 M | 1902.8 KiB | 387.2 KiB | 52.0 MiB |
| `dyn Trait<A>` (f606b34) | 663.8 M | 620.8 M | 3893.0 M | 1902.8 KiB | 387.2 KiB | 51.8 MiB |
| 境界に合わせた整数リテラル、`Box` の比較 (60cbe60) | 661.0 M | 621.0 M | 3893.0 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| `()` を返す `impl Trait` (e7fdc21) | 663.2 M | 620.8 M | 3893.0 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| 今: ユーザーのコードが定義しなおせる std の型名 | 663.8 M | 620.8 M | 3893.0 M | 1902.8 KiB | 387.2 KiB | 51.9 MiB |
| **借用検査を入れた後からの変化** | **-11.3%** | **-12.2%** | **-2.0%** | **0%** | **0%** | **-32%** |

LSP (zustc 自身のソースに対する `bench/lsp_parity.py` の p50。実時間):

| 時点 | definition | hover | completion | references | 起動から索引ができるまで |
|---|---:|---:|---:|---:|---:|
| 借用検査を入れた後 | 0.65 ms | 0.47 ms | 1.28 ms | 3.36 ms | 135 ms |
| 互換性の作業 (03367bc) | 0.53 ms | 0.43 ms | 1.09 ms | 3.06 ms | 116 ms |
| prelude の選別の高速化 (3a1fd22) | 0.58 ms | 0.47 ms | 1.37 ms | 3.45 ms | 109 ms |
| async (e598fdf)、同じマシンでの比較 | 0.42 ms | 0.34 ms | 0.90 ms | 1.78 ms | 277 ms (2 つの対象の合計) |
| ブロックの中のアイテム (cbcd294) | 0.59 ms | 0.49 ms | 1.26 ms | 3.52 ms | 109 ms |
| 推論と呼び出しの引数の借用検査 (8e797a9) | 0.55 ms | 0.45 ms | 1.15 ms | 3.39 ms | 115 ms |
| 組み込みマクロ (1501b30) | 0.53 ms | 0.42 ms | 1.21 ms | 3.04 ms | 108 ms |
| トレイトの `impl Trait`、マクロの追加 (8cb2f88) | 0.59 ms | 0.49 ms | 1.22 ms | 3.76 ms | 145 ms (1501b30 と同じマシンで比べると 281 ms と 285 ms、2 つの対象の合計) |
| 型推論の `assert!` (4d210d2) | 0.63 ms | 0.51 ms | 1.32 ms | 3.41 ms | 111 ms |
| 型推論の `assert_eq!` (441d0c2) | 0.61 ms | 0.47 ms | 1.27 ms | 3.49 ms | 114 ms |
| レジスタの値の借用 (fb2f2f2) | 0.57 ms | 0.48 ms | 1.19 ms | 3.74 ms | 110 ms |
| `try_from`、`::std` のパス、std の関数の追加 (096a6bc) | 0.57 ms | 0.48 ms | 1.22 ms | 3.47 ms | 122 ms |
| ビルドごとの表を作らない prelude の選別 (f077e7c) | 0.55 ms | 0.45 ms | 1.30 ms | 3.26 ms | 111 ms |
| `dyn Trait<A>` (f606b34) | 0.51 ms | 0.43 ms | 1.17 ms | 3.39 ms | 109 ms |
| 境界に合わせた整数リテラル、`Box` の比較 (60cbe60) | 0.60 ms | 0.60 ms | 1.23 ms | 3.25 ms | 109 ms (f606b34 と同じマシンで比べると hover は 0.32 ms と 0.35 ms、起動は 275 ms と 287 ms) |
| `()` を返す `impl Trait` (e7fdc21) | 0.94 ms | 0.56 ms | 1.30 ms | 11.57 ms | 115 ms (60cbe60 と同じマシンで 8 回交互に比べると definition は 0.55 ms と 0.55 ms、references は 2.34 ms と 2.35 ms、起動は 291 ms と 299 ms) |
| 今: ユーザーのコードが定義しなおせる std の型名 | 0.59 ms | 0.46 ms | 1.03 ms | 12.17 ms | 141 ms (さらに 3 回測ると 109、112、129 ms) |

どの変更も、これらが悪くならないときだけ残す。全体の計測で LSP の時間が上がったときは、同じマシンで 2 つのビルドを
交互に走らせて比べ直してから、揺れかどうかを決める。e598fdf の行はそうして比べた値で、起動の時間は 2 つの対象のファイルの合計。ほかの行は `bench/ci_bench.py` の全体の計測。

main に入るたびに測った数字の推移は [ベンチマークのダッシュボード](https://mizchi.github.io/zust/dev/bench/) にある
(GitHub Actions の共有のランナーで測るので、時間は揺れる。命令数の系列の方が差を見やすい)。

### rustc のテスト集をどれだけ通るか

rustc 自身の UI テスト (rust-lang/rust 1.100.0 の `tests/ui`、`python3 bench/rust_ui.py`) を zustc で走らせた結果。
zustc に求められるもの (nightly の機能・補助のクレート・特別なフラグが要らないもの) だけを数える:

| 種類 | 対象 | 通った |
|---|---:|---:|
| run-pass (ビルドでき、実行して同じ出力) | 2413 | 1441 (59.7%) |
| check-pass (`zustc check` が通す) | 1807 | 1143 (63.3%) |
| build-pass | 135 | 82 (60.7%) |
| compile-fail (注釈の行で拒む) | 7358 | 2815 (38.3%) |

分野ごとの内訳はチェックリストの形で [SUPPORTED.ja.md](SUPPORTED.ja.md) にある。

## 必要なもの

- Linux x86-64
- Rust のツールチェイン (edition 2024 が使えるもの。rustup の stable で可)
- C コンパイラのドライバ `cc` (リンク行を作るのに使う) と、リンカ
  - リンカは wild / mold / ld.lld / ld のどれか。PATH にある順に wild > mold > ld.lld > ld で選ぶ。
- zust-analyzer で std の定義を引くには rust-src: `rustup component add rust-src`

## インストール

```bash
git clone https://github.com/mizchi/zust.git
```

```bash
cd zust
```

```bash
cargo install --path .
```

```bash
cargo install --path crates/analyzer
```

`zustc` と `zust-analyzer` が `~/.cargo/bin` に入る。インストールせずに `cargo build --release --workspace` で
`target/release/` に作ってもよい。

## zustc の使い方

### 1 ファイルのプログラム

```bash
zustc hello.rs -o hello
```

```bash
./hello
```

`-o` を省くと、入力と同じ場所に拡張子なしの名前 (`hello`) で出す。出力の横にキャッシュ (`hello.crcache2`)、オブジェクト
(`hello.o`)、リンク行 (`hello.link`) を置く。2 回目からは変わった関数だけをコンパイルし直す。何も変わっていなければ
リンクもしない。

`mod x;` は `x.rs` / `x/mod.rs` を読む。

### 検査だけ

```bash
zustc check hello.rs
```

コード生成の手前 (型の検査) までして、すべての診断を出す。到達しない関数や、総称関数の要求されたインスタンスも検査する。

### 複数クレート

クレートを 1 行に 1 つ書いたファイルを渡す。書き方は `lib|bin <名前> <根のファイル> [依存するクレート..]` で、パスはこのファイルからの相対:

```text
lib util util/src/lib.rs
lib geom geom/src/lib.rs util
bin app app/src/main.rs geom util
```

```bash
zustc build workspace.txt -o target
```

クレートごとにオブジェクトとキャッシュを作ってリンクする。依存されるクレートの関数の本体を変えても、使う側はコンパイルし直さない
(リンクだけ)。例は `tests/workspace/`。Cargo.toml は読まない。

### オプション

| オプション | 意味 |
|---|---|
| `-o PATH` | 出力のパス (`build` では出力のディレクトリ) |
| `-O` | 最適化する: 小さい関数のインライン展開と Cranelift の `speed`。オーバーフローの検査をしない |
| `--backend=cranelift\|minclif` | コード生成 (既定 cranelift。環境変数 `ZUSTC_BACKEND` でも) |
| `--linker=wild\|mold\|lld\|bfd\|PATH` | リンカを選ぶ |
| `--features=a,b` | `#[cfg(feature = "..")]` を有効にする |
| `-j N` | コード生成のスレッド数 |
| `--time` | 段ごとの時間とピーク RSS を出す |
| `--no-cache` | キャッシュを読まず、書かない |

panic は rustc と同じ形 (`thread 'main' panicked at src/main.rs:行:列:` とメッセージ、終了コード 101) で出る。
オーバーフローは rustc の debug ビルドと同じく検査する (`-O` では検査しない)。

## zust-analyzer の使い方

stdio で話す LSP サーバ。エディタの LSP クライアントから `zust-analyzer` を起動する。Cargo.toml のあるディレクトリを
ワークスペースの根として、クレート (lib / bin / tests / examples / build.rs と `mod` で辿れるファイル)、std
(rust-src)、Cargo.toml に書いた依存クレート (版は Cargo.lock から。`~/.cargo/registry/src` にあるもの) を索引する。

### Neovim (0.11 以降)

```lua
vim.lsp.config('zust_analyzer', {
  cmd = { 'zust-analyzer' },
  filetypes = { 'rust' },
  root_markers = { 'Cargo.toml', '.git' },
})
vim.lsp.enable('zust_analyzer')
```

### Helix (`languages.toml`)

```toml
[language-server.zust-analyzer]
command = "zust-analyzer"

[[language]]
name = "rust"
language-servers = ["zust-analyzer"]
```

他のエディタでも、任意のコマンドを LSP サーバとして起動できるクライアントなら使える。

### 対応している機能

| 機能 | 対応 |
|---|---|
| documentSymbol / workspace/symbol | ○ |
| definition / references / documentHighlight | ○ |
| hover / completion / signatureHelp | ○ |
| rename / prepareRename | ○ (ワークスペースの中だけ書き換える) |
| publishDiagnostics | ○ (zustc の check の診断。zustc が扱えない書き方には誤った診断が出ることがある) |
| semanticTokens / inlayHint / codeAction / formatting | × |

ファイルの同期は全文 (didChange で本文全体を受け取る)。

### 設定 (環境変数)

| 変数 | 意味 |
|---|---|
| `ZUST_ANALYZER_MEM_MB` | ワークスペースのファイルの本文をメモリに持つ上限 (MB、既定 4)。超えた分は必要なときに読み直す |
| `ZUST_ANALYZER_NO_STD` | std の索引を作らない |
| `ZUST_ANALYZER_NO_DEPS` | 依存クレートの索引を作らない |
| `ZUST_ANALYZER_SYSROOT` | std のソースを探す sysroot (既定は `rustc --print sysroot`) |

rust-analyzer との違いで分かっているものは [TODO.md](TODO.md) にある。

## ドキュメント

- [docs/ja/language.md](docs/ja/language.md): zustc が扱える Rust のサブセットと、扱えないもの
- [docs/ja/architecture.md](docs/ja/architecture.md): crate の構成、処理の流れ、インクリメンタルビルドのしくみ、値の表現
- [docs/ja/reports/](docs/ja/reports/README.md): 作業の記録 (日付順のレポート。ベンチマークの結果、各段階でやったこと)
- [CONTRIBUTING.ja.md](CONTRIBUTING.ja.md): 開発のしかた (ビルド、テスト、ベンチマーク、レポートの書き方)
- [SUPPORTED.ja.md](SUPPORTED.ja.md): 対応しているもの、目処がついているもの、パフォーマンスが悪化しそうなもの、未着手のもの
- [TODO.md](TODO.md): これからやること (英語)

## ライセンス

Rust 本体と同じく、次のどちらかを選んで使える (デュアルライセンス)。

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

明示しない限り、このリポジトリに送られた貢献は、Apache-2.0 の定義に従い、追加の条件なしに上の 2 つのデュアルライセンスで
提供されたものとする。

`cratebench/vendor/` の中のクレート (ベンチマーク用の git submodule) は、それぞれの上流のライセンスに従う。
