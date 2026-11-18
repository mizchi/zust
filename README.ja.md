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
> 借用検査などはしないので、rustc の代わりに正しさを確かめる道具ではない。zust-analyzer は rust-analyzer の一部の機能だけ。

## どのくらい速いか

`python3 bench/vs_rustc.py` の 1000 関数のプログラム (gen1000) の場合 (2026-11-15、4 コア):

| | cold | 1 か所編集 | キャッシュのディスク |
|---|---:|---:|---:|
| zustc | 118 ms | 28 ms | 105 KiB |
| rustc `-C incremental` | 827 ms | 417 ms | 22 MiB |

他のプログラムの結果は [docs/reports/2026-11-15-15-status.md](docs/ja/reports/2026-11-15-15-status.md) にある。

zust-analyzer と rust-analyzer を、zustc 自身のソース (40 か所の問い合わせ) で比べた (2026-11-18、`python3 bench/lsp_parity.py`):

| | 起動から名前が引けるまで | メモリ | references の p50 | references の数が rust-analyzer と同じ |
|---|---:|---:|---:|---:|
| zust-analyzer | 129 ms | 66 MB | 4.2 ms | 97% |
| rust-analyzer | 5594 ms | 2.4 GB | 24.1 ms | (基準) |

詳しくは [bench/lsp_parity_result.md](bench/lsp_parity_result.md)。

main に入るたびに測った数字の推移は [ベンチマークのダッシュボード](https://mizchi.github.io/zust/dev/bench/) にある
(GitHub Actions の共有のランナーで測るので、時間は揺れる。命令数の系列の方が差を見やすい)。

### rustc のテスト集をどれだけ通るか

rustc 自身の UI テスト (rust-lang/rust 1.100.0 の `tests/ui`、`python3 bench/rust_ui.py`) を zustc で走らせた結果。
zustc に求められるもの (nightly の機能・補助のクレート・特別なフラグが要らないもの) だけを数える:

| 種類 | 対象 | 通った |
|---|---:|---:|
| run-pass (ビルドでき、実行して同じ出力) | 2413 | 1120 (46.4%) |
| check-pass (`zustc check` が通す) | 1807 | 831 (46.0%) |
| build-pass | 135 | 64 (47.4%) |
| compile-fail (注釈の行で拒む) | 7358 | 2291 (31.1%) |

zustc は借用検査をしないので、compile-fail の多くはもともと対象外の性質のもの。

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
