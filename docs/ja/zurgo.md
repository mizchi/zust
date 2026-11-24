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
| `zurgo test [フィルタ] [-- 引数]` | テストをビルドし、テストごとに別のプロセスで、すべてを同時に動かす (下で述べる) |
| `zurgo metadata` | パッケージのグラフを、`cargo metadata --format-version 1` が書く形で出す |
| `zurgo clean` | `target/zust` を消す |

オプション:
- `--manifest-path <Cargo.toml>`: パッケージ。なければ、今のディレクトリから上へ、いちばん近い Cargo.toml を使う。
- `-r` / `--release`: zustc の `-O`。
- `-F` / `--features a,b`、`--all-features`、`--no-default-features`。`dep/feat` は依存の feature を指す。
- `--target <triple>`: `x86_64-unknown-linux-gnu` (既定) か `wasm32-unknown-unknown`。
  - wasm32 では、bin ごとに 1 つの wasm モジュール `target/zust/wasm32-unknown-unknown/<profile>/<bin>.wasm` になる。
  - `zurgo run` は cargo と同じようにそれを動かす: `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER` のプログラム
    (空白で分ける) に、モジュールと引数を渡す。このリポジトリでは、ランナーは `node tests/wasm/run.mjs`。
  - ビルドスクリプトは、今までどおりホスト向けにビルドし、ホストで動かす。
- `--bin <名前>`、`-j <n>`、`--time` (zustc の時間)、`--no-cache`。
- `--build-script-compiler auto|zustc|rustc`: ビルドスクリプトを何でコンパイルするか (下に)。既定の `auto` は
  zustc を試し、だめなら rustc を使う。
- `-v`: zurgo が zustc に渡すワークスペースを、`zustc build` の形で出す。`target/zust/<profile>/zurgo-ws.txt` にも
  書くので、`zustc build` で同じビルドを繰り返せる。クレート名と実行ファイルの名前が違う bin (パッケージの lib と
  bin はパッケージの名前を共有するので、bin のクレートは `<名前>-bin` になる) には `exe=<名前>` が付く。クレートごとの、
  ビルドスクリプトが与える cfg と、`env!` の環境 (`CARGO_PKG_*`、`OUT_DIR`) もそこに付く (`cfg=`、`env=`)。
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
    - メンバー自身の Cargo.toml を渡したとき (`--manifest-path`) も、メンバーが受け継ぐフィールド;
    - コマンドが扱うパッケージ: メンバー自身のマニフェストはそのメンバーを選び、ワークスペースの根のマニフェストは
      `default-members` (グロブも)、なければ根のパッケージ、それもなければすべてのメンバーを選ぶ。`--features` と
      `default-run` はその選んだものに効く。
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

## ビルドスクリプト

パッケージの `build.rs` は、cargo と同じく、そのクレートをビルドする前に実行する:

- **コンパイル。** ホスト向けのバイナリで、パッケージのビルド用の依存を使う。
  - `auto` (既定): zustc が、ビルド用の依存とスクリプトを 1 つの zustc のワークスペースとしてビルドする。zustc が
    できないとき (zustc にない std の関数、proc-macro のビルド用の依存) は、代わりに rustc がビルドする: ビルド用の
    依存を rlib に (proc macro は dylib に) してから、スクリプトを。
  - `-v` は、どちらがスクリプトをビルドしたか、zustc がなぜできなかったかを出す。zurgo はソースが変わるまでその
    選択を覚えておく。
- **実行。** パッケージのディレクトリで、cargo が設定する変数を付けて実行する:
  - `OUT_DIR`、`TARGET`、`HOST`、`NUM_JOBS`、`OPT_LEVEL`、`DEBUG`、`PROFILE`;
  - `CARGO_MANIFEST_DIR`、`CARGO_MANIFEST_PATH`、`CARGO_MANIFEST_LINKS`、`CARGO_PKG_*`;
  - `CARGO_FEATURE_<名前>` と `CARGO_CFG_<キー>` (ターゲットの rustc の値);
  - `RUSTC`、`RUSTDOC`、`CARGO`、`CARGO_ENCODED_RUSTFLAGS`;
  - 依存する `links` のパッケージからの `DEP_<LINKS>_<キー>`。

  `RUSTC` は本物の rustc なので、スクリプトの試しのコンパイルと版の確認は、cargo のときと同じものを見る。
- **出力** (`cargo:` と `cargo::` の形):
  - `rustc-cfg` と `rustc-env` は、パッケージのクレートに渡す;
  - `warning` は出し、`error` はビルドを止める;
  - `metadata` (古い形では、ほかのキーすべて) は、依存する側のスクリプトに `DEP_<LINKS>_<キー>` として渡す;
  - リンクの指示 (`rustc-link-lib`、`rustc-link-search`、`rustc-link-arg`、`rustc-flags`) は取っておくが、zustc は
    まだ使わない: 警告でそう言う。
- **もう一度実行するとき。** 出力は `target/zust/<profile>/build/<パッケージ>-<ハッシュ>/run` に取っておく。スクリプトが
  変わったとき、cargo の変数が変わったとき、`rerun-if-changed` / `rerun-if-env-changed` が名指しするファイルか変数が
  変わったときに、もう一度実行する。どちらも出さないスクリプトは、cargo と同じく、パッケージのどれかのファイルが
  変わったときに実行する。
  - 実行している間に依存するファイルが変わったとき (消えたときも: ディレクトリの時刻でわかる)、ディレクトリが
    読めなかったとき、名指しするファイルがないとき (cargo はそういうスクリプトを毎回実行する) は、その結果を
    取っておかない。
  - スクリプトをもう一度実行する前に、前の結果を消す。なので、失敗した実行が使い回せるものを残すことはない。
  - ターゲットディレクトリを共有する 2 つの zurgo のプロセスは、cargo と同じく順に動く: ビルドは最初のビルド
    スクリプトから最後のクレートのコンパイルまでそのディレクトリをロックするので、スクリプトが書いた `OUT_DIR` が
    そのままクレートの読むものになる。
- **rustc のビルド** (ホスト向けのライブラリとスクリプト) は、rustc (`rustc -vV`)、ソース、依存の成果物、それらの
  ビルドスクリプトが `OUT_DIR` に書いたファイルが同じ間は取っておく。

どのクレートも、`env!` / `option_env!` のために cargo の変数を受け取る: `CARGO_PKG_*`、`CARGO_MANIFEST_DIR`、
`CARGO_CRATE_NAME`、bin なら `CARGO_BIN_NAME`。`OUT_DIR` とスクリプトの `rustc-env` はその上に加わる。

## proc-macro の依存

proc macro は動く (mizchi/zust#29、方式 A)。rustc が proc-macro の依存を dylib に、その依存を rlib にビルドする。
それらのビルドスクリプトが先に走り、ビルドしたものはビルドスクリプトのホスト用ライブラリと同じように取っておく。
zustc には、そのクレートを、その名前のスタブのライブラリと、横に置いた dylib として渡す (ワークスペースのファイルの
`pm=PATH`)。ツールチェインの proc-macro サーバ (rustc の sysroot の `libexec/rust-analyzer-proc-macro-srv`。`rustc`
コンポーネントに入っている) が dylib を読み込み、使っているところを 1 つずつ展開する:
- **何が「使っているところ」か。** zustc は名前で見つける。前に zurgo の検査が見つけていたのと同じ:
  - 属性マクロ: `#[wasm_bindgen]`、`#[wasm_bindgen_macro::wasm_bindgen(..)]`、または `use` で取り込んだ名前
    (名前を変えたものも);
  - derive: `#[derive(.., Describe)]`。言語の derive と同じ名前のものは、std の外からその名前を取り込んでいるとき
    だけ数える;
  - 関数のようなマクロ: `pm::html!(..)`。アイテム・文・式のどの位置でもよい。その出力は、`macro_rules!` の呼び出しを
    展開したものと同じところに置く。
  - 数えるのは、そのクレートからたどり着く proc-macro のクレートだけ: 依存、またはそのライブラリが再エクスポート
    するもの。std やツールへのパス (`#[rustfmt::skip]`) は、proc macro のものではない。
  - **どのクレートのものか。** パスは、それが始まるクレートのマクロ: `pm::make!()`、またはライブラリ `re` が
    再エクスポートしているなら `re::make!()` (その `pub use`。名前の変更やグロブも)。そうしたパスは、
    `macro_rules! make` があっても proc macro のもので、`stringify!(..)` の中では書いたまま残す。`use` で取り込んだ
    名前は (波括弧の中でも)、その `use` が始まるクレートのマクロ。`use` はクレートごとに 1 つの表なので、2 つの
    モジュールが別々のクレートのマクロを同じ名前で取り込むと、区別しない。
- **マクロが受け取るもの。**
  - アイテムのトークン。cfg は評価済みで、doc コメントは `#[doc = ".."]` になっている。
  - そのクレートのコンパイル時の環境 (`CARGO_PKG_*`)。`CARGO_MANIFEST_DIR` をカレントディレクトリにする。
  - 属性マクロのアイテムには、その属性を含めない。derive のアイテムには、その `#[derive]` を含めず、アイテムは
    ほかの derive を持ったまま残る。
- **出力はもう一度読む**。なので、出力が使うマクロも順に展開する (rustc と同じく外側から)。マクロの呼び出しの引数は、
  その呼び出しを展開するまで読まない。
- **キャッシュ。** 展開したものは、そのクレートのほかの前処理と一緒に取っておく。dylib をビルドし直したら、展開も
  やり直す。
- **まだのもの。**
  - span はどれも呼び出したところのもの。zustc は proc macro の出力の衛生性 (hygiene) を持たず、展開したものの中の
    エラーは使っているところを指す。
  - ビルドスクリプトのビルド用の依存に proc macro があるもの: そのスクリプトは rustc がビルドする。

サーバがないとき (`ZUSTC_PROC_MACRO_SRV` が空で設定されている、またはツールチェインにない) は、proc-macro の依存は
その名前のスタブのライブラリだけになる。なので、それを再エクスポートするだけのクレートはビルドできる
(`pub use thiserror_impl::*;` や、serde の `derive` feature)。zustc は知らない属性を黙って捨てるので、そのときは
proc macro を使うと、何もビルドする前にエラーになる:
- ビルドのどれかのクレートが、スタブの derive (`#[derive(thiserror::Error)]`) か属性マクロ (`#[wasm_bindgen]`。
  名前を変えたもの `use wasm_bindgen::prelude::wasm_bindgen as wb;` も) を使っていれば、#29 を示すエラーになる。
  - それぞれのクレートは zustc が読むとおりに読む: そのモジュールの木のファイルを、その feature と cfg で。なので、
    たどり着かないファイルの中、無効な cfg の下、呼ばれもエクスポートもされない `macro_rules!` の本体で使っている
    ものは数えない。
  - proc-macro のクレートのルートは、その feature で読む。なので、`#[cfg_attr(feature = "x", proc_macro_attribute)]`
    は、その feature が有効なら数える;
- 関数のようなマクロ (`html!(..)`) は、#29 を示す `compile_error!` に展開する。

## テスト

`zurgo test` は libtest ではなく nextest の方式で動く (mizchi/zust#96)。選んだパッケージのテストのクレートをビルドする:
lib の単体テスト、各 bin の単体テスト、各結合テスト (`[[test]]`、`tests/*.rs`、`tests/*/main.rs`) を、dev-dependencies
込みで。各実行ファイルのテストを列挙し、テストごとに別のプロセスで、コアに分けて同時に動かす。

```bash
zurgo test
```

```bash
zurgo test --lib parse -- --include-ignored
```

- テストのクレートは (後述の `harness = false` のものを除き) どれも `zustc --test` でビルドする。`#[cfg(test)]` のアイテムを残し、`#[test]` の関数を集め (`macro_rules!` が作るものも:
  展開したソースから集める)、
  ハーネスの `main` を足す (クレート自身の `main` は呼ばれない)。ハーネスがするのは、テストの列挙と、名前を
  指定されたテストを 1 つ動かすことだけ:
  - panic は 101 で終わり、`Err` を返す `Result` のテストは 1 で終わる。
  - `#[should_panic]` のテストは子プロセスで動かし、その終了コードと stderr から、`expected` の文字列で panic
    したかを見る。

  なので、コンパイルしたプログラムに `catch_unwind` もスレッドも出力の捕捉もいらない。コマンドラインは libtest のもの
  (`--list --format terse`、`--exact`、`--ignored`、`--include-ignored`、`--skip`、フィルタ) なので、cargo-nextest も
  これらの実行ファイルを動かせる。実行ファイルを引数なしで直接動かすと、テストを 1 つずつ順に、それぞれ子プロセスで
  動かし、libtest の形で報告する。
- テストは、プロセスが 0 で終われば通ったものとする。ランナーは libtest のコマンドラインで話すので、
  `cargo test --no-run` がビルドする実行ファイルも動かせる。テストは 2 つの実行の結果を比べる
  (`crates/zurgo/tests/zurgo.rs`)。
- `cratebench/test.py` は、cratebench のクレート自身のテスト (上流のリポジトリのもの) を `zurgo test` と
  `cargo test` で動かし、テストごとに比べる。CI は一致したクレート (`cratebench/test_passing.txt`) を確かめる。
  `cratebench/test_result.md` にはすべてのクレートの結果があり、zurgo がまだビルドできないものは最初のエラーを載せる。
- テストは cargo と同じく、パッケージのディレクトリで動く。結合テストは、パッケージのバイナリを
  `CARGO_BIN_EXE_<name>` で見つける。
- 出力: テストが終わるたびに 1 行 (`PASS [ 0.012s] (3/9) pkg (lib) tests::adds`、または `FAIL`・`TIMEOUT`・`SIGNAL`)。
  失敗したものはその後に stdout と stderr を出す。最後にまとめを出す。テストが失敗したら終了コードは 101。
- 長いテストから始める。順番は `target/zust/<profile>/test-times` に残した前回の時間で決める。長いテストが最後に
  始まると、終わりにそれだけが残って動くことになる。
- オプション:
  - `--lib`、`--bin <name>`、`--bins`、`--tests`、`--test <name>` でターゲットを選ぶ: それだけをビルドする
    (`CARGO_BIN_EXE_` のために bin も)。
  - `--no-run` はビルドして、実行ファイルを並べるだけ。
  - `--fail-fast` は、失敗の後に新しいテストを始めない (既定ではすべてのテストを動かす。`--no-fail-fast` も受け付ける)。
- `--` の後: `<filter>`、`--exact`、`--skip <s>`、`--ignored`、`--include-ignored`、`--list`、`--test-threads <n>`
  (既定はコアの数)。`--nocapture` と `--show-output` も受け付ける: 失敗したテストの出力は必ず出す。
- 60 秒たっても動いているテストには `SLOW` と出し、300 秒たっても動いているものは止めて失敗にする
  (`ZURGO_TEST_SLOW`、`ZURGO_TEST_TIMEOUT`。秒で)。
- ターゲットの `test = false` は、そのテストを外す (cargo と同じく、名前で選んだとき (`--lib`、`--bin x`、`--test x`) は
  動かす)。`harness = false` のターゲットはハーネスなしでビルドし (自分の `main`。cargo と同じく `cfg(test)` は付ける)、
  ターゲットの名前の 1 つのテストとして動かす: 0 で終われば通る。`zurgo metadata` は結合テストを
  cargo と同じく並べる。
- `--target wasm32-unknown-unknown`: テストのクレートはどれも wasm のモジュールになり、各テストを
  `$CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER <module> <name> --exact` で動かす (このリポジトリではランナーは
  `node tests/wasm/run.mjs`)。モジュールは子プロセスを起こせないので、`#[should_panic]` のテストはモジュールの中で動き、
  ランナーがその終了コード (101) と panic のメッセージを確かめる。そこでは算術の検査はメッセージなしで trap するので、
  `expected` はそれに合わない。
- まだのもの: doc テストとベンチマーク。

## まだのもの (mizchi/zust#28)

- **1 つのライブラリの 2 つの版**と、**名前を変えた依存**はエラーになる: zustc のワークスペースでは、クレートごとに
  名前が 1 つである。
- **ほかのソース:** git の依存、ほかのレジストリ、ソースの置き換え。
- **`[patch]`、自前のリゾルバ、ダウンロード、`doc` はない。**
- **edition:** zustc には edition の設定がない: どの edition が受け付けるものも受け付ける。なので、クレートの
  edition は渡さない (ビルドスクリプトをビルドする rustc には、それぞれのクレートの edition を渡す)。
