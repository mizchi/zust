# zurgo M1: ビルドスクリプト、`env!`、proc-macro のスタブ

日付: 2026-11-22

M0 は Cargo のパッケージを、ビルドスクリプトを実行せずにビルドし、proc-macro の依存があれば止まった。これは
mizchi/zust#28 の 2 つめのマイルストーンで、wasm-bindgen のロードマップ (mizchi/zust#38) の W0 の始まりである。
使い方: [zurgo.md](../zurgo.md)。

## 1. 変えたもの

- **zustc: クレートごとの cfg とコンパイル時の環境。**
  - `env!` / `option_env!` は、ファイルを読むときに展開する。まずクレートの環境を、次に zustc 自身の環境を読む
    (rustc が自分の環境を読むのと同じ)。
  - なので `include!(concat!(env!("OUT_DIR"), "/x.rs"))` が動く。
  - ワークスペースのクレートはそれぞれ、cfg (`cfg=name`、`cfg=name=value`) と環境変数 (`env=KEY=VALUE`) を持てる。
- **zurgo: ビルドスクリプト。**
  - ビルドスクリプトを、そのビルド用の依存と一緒に、zustc でできれば zustc で、できなければ rustc でコンパイルする。
  - cargo の変数を付けて実行し、`cargo:` / `cargo::` の出力を読む。
  - 依存するものが変わったときだけ、もう一度実行する。
- **zurgo: どのクレートにも cargo の変数。** `CARGO_PKG_*`、`CARGO_MANIFEST_DIR`、`CARGO_CRATE_NAME`、bin なら
  `CARGO_BIN_NAME`。
- **zurgo: proc-macro の依存はスタブ。** proc-macro のクレートは、その名前の空のライブラリとしてビルドするので、それを
  再エクスポートするだけのクレートはビルドできる。そのマクロのどれかを使うのは、mizchi/zust#29 を示すエラーになり、
  属性を黙って捨てることはない。
- **途中で見つけた zustc の直し:**
  - std の書式のマクロの引数の中の `cfg!` (`println!("{}", cfg!(unix))`) を、評価していなかった;
  - `compile_error!`;
  - `dyn Error + Send + Sync + UnwindSafe` (自動 trait として数えていたのは `Send` と `Sync` だけだった);
  - `std::path::Display`;
  - `fmt::Pointer`。

## 2. 決めたこと

mizchi/zust#28 で未決だった問い:

- **ビルドスクリプトの `RUSTC` は本物の rustc にした** (問い 1)。スクリプトの試しのコンパイル (thiserror と anyhow
  の nightly の試し、autocfg) と版の確認 (rustversion、version_check) は、cargo のときと同じものを見る。クレートは
  cargo でビルドしたときと同じ道を通る。zustc は新しい安定版の rustc に従うので、その道こそ zustc が受け付けるべき
  ものである。
- **ビルドスクリプトのコンパイラは zustc で、だめなら rustc にした** (問い 3、`--build-script-compiler auto`)。
  - 試したスクリプトのうち、zustc がコンパイルできるのはテストのパッケージの `gen` のものだけである。thiserror、
    anyhow、libc、serde_core と、テストのパッケージのアプリのスクリプトは、zustc にない std の関数で止まる:
    `env::vars()`、`OsString` を取る `Command`、`process::Output`、`fs::create_dir`。
  - それらは rustc がビルドするので、どのスクリプトも実行できる。
  - zurgo は、どちらがスクリプトをビルドしたかを、そのソースが変わるまで覚えておく。なので、zustc がビルドできない
    スクリプトを、ビルドのたびに試し直すことはない。
- **proc-macro の依存は、確かめた上でスタブにした。**
  - zustc は知らない属性を黙って捨てるので、スタブだけでは、属性マクロを使うコードを間違ってコンパイルしうる。
  - そこで zurgo は、proc-macro のクレートが定義するマクロ (`#[proc_macro_derive(..)]`、`#[proc_macro_attribute]`、
    `#[proc_macro]`) を読む。ビルドのどれかのクレートのソースがその derive か属性を使っていれば、何もコンパイル
    する前にビルドを止める。
  - 関数のようなマクロは `compile_error!` に展開する。

## 3. 結果

`crates/zurgo/tests/zurgo.rs`:

- **`build_scripts_match_cargo`。** テストのパッケージを、`rustc` と `auto` の両方で `cargo run` と比べる。出力は
  同じである。そのパッケージには、次のものがある:
  - ビルド用の依存を使い、`DEP_GEN_*` を読み、`OUT_DIR` にコードを書くビルドスクリプト;
  - `rustc-cfg` (`name="value"` も) と `rustc-env`;
  - pre-release の版の `CARGO_PKG_*`、`CARGO_CFG_*`、`PROFILE`;
  - `links` を持つ依存。そのスクリプトは、2 つの形で metadata を出し、feature によって決まる cfg を出す。

  テストは、2 回めのビルドがどのスクリプトも実行しないこと、アプリのスクリプトが `rerun-if-changed` で名指しする
  ファイルを変えると、そのスクリプトだけを実行することも確かめる。失敗するスクリプトは、その出力を付けてビルドを
  止める。
- **thiserror 2.0.21 のライブラリはビルドできる。** それに依存するアプリは、cargo でビルドしたものと同じものを出す
  ので、`registry_crates_run_like_cargo` に加えた。ライブラリに要るもの:
  - `CARGO_PKG_VERSION_PATCH` から `OUT_DIR/private.rs` を書き、`$RUSTC` を試すビルドスクリプト;
  - `include!(concat!(env!("OUT_DIR"), "/private.rs"))`;
  - thiserror-impl のスタブ;
  - 上の zustc の 4 つの直し。

  `#[derive(thiserror::Error)]` は #29 のエラーになる。
- **`errors`。** 使われない proc-macro の依存はビルドできる。その属性を使えば #29 のエラーで、関数のようなマクロも
  同じである。

いまビルドスクリプトを実行できるクレートと、次に止まる所。どれも zustc 自身の足りない所である:

| クレート | ビルドスクリプト | クレートそのもの |
|---|---|---|
| thiserror 2.0.21 | 実行できる (rustc) | ビルドできる |
| anyhow 1.0.104 | 実行できる (rustc) | `error.rs:808`: ライフタイムの指定がない (E0106) |
| libc 0.2.191 | 実行できる (rustc) | `prelude!` マクロの中の `mod types;` |
| serde_core 1.0.229 | 実行できる (rustc) | `crate_root!` マクロの中の `mod de;` |

あとの 2 つでは、rustc はモジュールのファイルを、マクロを展開した場所 (クレートの根) から探す。zustc はマクロを
定義したファイルのそばを探す。

## 4. 次

mizchi/zust#38 の W0 のために:
- wasm32 向けのワークスペースのビルド (zustc のワークスペースのビルドは x86-64 だけ);
- extern の名前とは別のクレートの同一性 (1 つのクレートの 2 つの版、名前を変えた依存);
- wasm-bindgen-shared までの途中にあるコンパイラの足りない所 (UI のレポートの 3、2.5、2.6 節)。

次に W1: proc macro そのもの (mizchi/zust#29)。ビルドスクリプトのための rustc の道は、すでに proc-macro のクレートを
dylib としてビルドする。それが #29 のバックエンド A である。
