# zurgo: Cargo のパッケージを zustc で

`zurgo` は、`cargo build` が rustc でビルドするように、Cargo のパッケージを zustc でビルドする。Cargo.toml と
Cargo.lock を読み、ソースを見つけ、cargo と同じように feature を有効にし、そのグラフを同じプロセスの中で zustc の
ワークスペースのビルドに渡す。`zurgo metadata` は `cargo metadata` の JSON を書く。計画と残り: mizchi/zust#28。

```bash
cargo install --path crates/zurgo
```

```bash
zurgo run
```

## コマンド

| コマンド | すること |
|---|---|
| `zurgo build` | マニフェストのパッケージ (仮想ワークスペースならすべてのメンバー) とその依存をビルドする |
| `zurgo run [-- 引数]` | ビルドしてから、バイナリを実行する (いくつかあるときは `--bin` で選ぶ) |
| `zurgo metadata` | パッケージのグラフを、`cargo metadata --format-version 1` が書く形で出す |
| `zurgo clean` | `target/zust` を消す |

オプション:
- `--manifest-path <Cargo.toml>`: パッケージ。なければ、今のディレクトリから上へ、いちばん近い Cargo.toml を使う。
- `-r` / `--release`: zustc の `-O`。
- `-F` / `--features a,b`、`--all-features`、`--no-default-features`。`dep/feat` は依存の feature を指す。
- `--target <triple>`: `x86_64-unknown-linux-gnu` (既定)。`wasm32-unknown-unknown` も受け付けるが、zustc の
  ワークスペースのビルドはまだそれを受け付けない。
- `--bin <名前>`、`-j <n>`、`--time` (zustc の時間)、`--no-cache`。
- `-v`: zurgo が zustc に渡すワークスペースを、`zustc build` の形で出す。`target/zust/<profile>/zurgo-ws.txt` にも
  書くので、`zustc build` で同じビルドを繰り返せる。クレート名と実行ファイルの名前が違う bin (パッケージの lib と
  bin はパッケージの名前を共有するので、bin のクレートは `<名前>-bin` になる) には `exe=<名前>` が付く。
- `--filter-platform <triple>` (metadata)。
- `--locked`、`--offline`、`--frozen` は受け付ける。zurgo はいつも Cargo.lock をそのまま使い、何もダウンロードしない。

出力は `target/zust/[<triple>/]<profile>/` に置き (`CARGO_TARGET_DIR` で `target` が変わる)、cargo 自身の出力とは
分ける。バイナリは `target/zust/debug/<名前>`。

## 読むもの

- **マニフェスト。**
  - `[package]`。ワークスペースのメンバーが受け継ぐフィールド (`version.workspace = true` など) も。
  - `[lib]` と `[[bin]]`、それに Cargo が自分で見つけるターゲット: `src/lib.rs` (`autolib = false` でなければ)、
    `src/main.rs`、`src/bin/*.rs`、`src/bin/*/main.rs` (`autobins = false` でなければ)。
  - bin の `required-features`: それが無効な間は、既定のビルドはその bin を入れず、名前で求めると何が足りないかを
    言う。`default-run` は `zurgo run` の bin を選ぶ。
  - 3 種類の依存。`[target.'cfg(..)']` の下のものも。名前の変更、`optional`、`default-features`、`features` も。
  - `[features]` と `[workspace]`:
    - `*` のグロブを使えるメンバー。ディレクトリごとに 1 つ;
    - ワークスペースのディレクトリの中の path の依存。cargo と同じく、並べなくてもメンバーになる;
    - `exclude`、`default-members`、`[workspace.dependencies]`;
    - メンバー自身の Cargo.toml を渡したとき (`--manifest-path`) も、メンバーが受け継ぐフィールド。
  - TOML のパーサは zurgo 自身のもの (`crates/zurgo/src/toml.rs`)。
- **Cargo.lock** (バージョン 1 から 4)。zurgo にはまだリゾルバがないので、版は lock のもの。lock がなければ、
  `cargo generate-lockfile` を実行するよう言う。
- **ソース。**
  - ワークスペースのメンバーと path の依存。
  - `$CARGO_HOME/registry/src` にある crates.io のパッケージ (`cargo fetch` がそこに置く)。
  - ダウンロードされていないパッケージ (別のプラットフォームのもの) は、cargo の index のキャッシュ
    (`registry/index/*/.cache`) から、その依存と feature を読む。`metadata` にはそれで足りる。それをビルドするのは
    エラーで、`cargo fetch` を実行するよう言う。

## feature の有効にし方は cargo と同じ

- **`zurgo build`** は cargo の feature のリゾルバ (resolver 2 と 3) の規則を使う。ビルドに使うのはこれである:
  - ホスト向けにビルドするパッケージ (ビルド用の依存、proc macro、それらが使うもの) は、自分の feature の組を持つ;
  - dev-dependencies は入れない;
  - `[target.'cfg(..)']` の依存は、それがある側のプラットフォームで確かめる;
  - 弱い `dep?/feat` は、ほかの何かが `dep` を有効にするまで待つ。
- **`zurgo metadata`** は `cargo metadata` が出すものを出す。それは cargo の依存のリゾルバから来る:
  - パッケージごとに feature の組は 1 つで、すべての種類、すべてのプラットフォームにわたる;
  - 弱い `dep?/feat` も、`dep` をグラフに入れる;
  - `--filter-platform` はどの辺を見せるかを選ぶだけである。辺は、その宣言のどれかが当てはまれば見せ、その
    `dep_kinds` にはすべての宣言を並べる。
- **どちらでも:**
  - `x/f` は依存 `x` を有効にし、`x` が optional なら、`x` という名前の feature があればそれも有効にする;
  - どの feature も `dep:` で名指ししない optional の依存には、その名前の暗黙の feature がある。

`crates/zurgo/tests/zurgo.rs` は、テストのパッケージと、cratebench/ui の dioxus と leptos のアプリ (344 と 180 の
パッケージ。wasm32 と x86-64 Linux の `--filter-platform` でも) について、`zurgo metadata` と `cargo metadata` が同じ
グラフを出すことを確かめる。dioxus では、zurgo は 39 ms、cargo は 320 ms かかる。

## まだのもの (mizchi/zust#28)

- **ビルドスクリプトは実行しない。** それぞれを警告で示す。それが設定する cfg と書くファイルはない。
- **proc-macro の依存はエラーになる** (mizchi/zust#29)。
- **1 つのライブラリの 2 つの版**と、**名前を変えた依存**はエラーになる: zustc のワークスペースでは、クレートごとに
  名前が 1 つである。
- **ほかのソース:** git の依存、ほかのレジストリ、ソースの置き換え。
- **`[patch]`、自前のリゾルバ、ダウンロード、`test` / `doc` はない。**
- **edition:** zustc はまだクレートごとの edition を受け付けない。
