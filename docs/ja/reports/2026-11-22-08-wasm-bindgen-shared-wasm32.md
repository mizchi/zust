# W0: wasm-bindgen-shared を zurgo で wasm32 向けに

日付: 2026-11-22

wasm-bindgen のロードマップ (mizchi/zust#38) の W0 は、wasm-bindgen-shared を zurgo で wasm32 向けにビルドすること
だった。wasm-bindgen-shared は、wasm-bindgen のうち proc macro のない部分である。W0 は終わった:
wasm-bindgen-shared 0.2.130 の公開 API を全部呼ぶアプリを `zurgo run --target wasm32-unknown-unknown` でビルドすると、
node の上で cargo のビルドと同じものを出す。前のマイルストーンは [zurgo M1 のレポート](2026-11-22-04-zurgo-build-scripts.md)。
使い方: [zurgo.md](../zurgo.md)。

## 1. 変えたこと

### 1.1 wasm32 向けのワークスペースのビルド

`zustc build --target wasm32-unknown-unknown` と `zurgo --target wasm32-unknown-unknown` は、"workspace builds are not
supported yet" で止まっていた。

- **問題。** x86-64 では、クレートごとに ELF のオブジェクトを作り、システムのリンカがそれらをつなぐ。ここには wasm の
  リンカがない: zustc のモジュールの書き出しは、すべての関数とデータからモジュールを一度に書く。
- **クレートの `.o` の中身 (wasm32)。**
  - クレートのオブジェクトは、その*部品*になった: モジュールの書き出しに渡すはずの関数とデータを直列化したもの
    (`zust-wasm-parts 1`)。
  - bin のリンクでは、自分と依存するクレートの部品を読み、クレートごとのローカルなシンボルに別々の名前
    (`name@crate`) を付ける。
  - それから、モジュールの書き出しを 1 回呼び、`<bin>.wasm` を書く。
- **キャッシュとインクリメンタルビルド**は x86-64 と同じように働く。1 つのクレートを編集すると、そのクレートの部品を
  書き直し、bin をリンクし直す。
- **`zurgo run`** は、cargo と同じように wasm の bin を動かす: `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER` の
  プログラムに、モジュールと引数を渡す。このリポジトリでは、ランナーは `node tests/wasm/run.mjs`。変数がなければ、
  そう言うエラーになる。
- **ビルドスクリプト**は、今までどおりホスト向けにビルドし、ホストで動かす。cargo と同じく、wasm32 の
  `CARGO_CFG_*` を見る。
- **wasm のバックエンド**には、byteorder が使う 128 ビット整数のバイトの入れ替え (`u128::swap_bytes`、`to_be`) が
  なかった。今は、2 つの 64 ビットの半分を、それぞれのバイトを逆にして入れ替える。
- **その入れ替えのテスト (examples/int128.rs) で見つかった 2 つの修正:**
  - minclif は 128 ビット整数の下位 64 ビットしか入れ替えず、上位の半分が失われていた。
  - 1 ファイルのビルドは、オブジェクトが変わらず実行ファイルがあればリンクを省いていた。その実行ファイルが
    オブジェクトより古くてもそうだった。`-o x.cold` と `-o x` は同じ `x.o` を使うので、tests/compare.sh の warm の
    ビルドが古い実行ファイルを残していた。今は、実行ファイルがオブジェクトより古くないことも要る。

### 1.2 途中のコンパイラの穴

wasm-bindgen-shared が当たったのは、はじめの 3 つ。ほかは、UI のレポートがその先のクレートについて挙げているもの
([3 節](2026-11-21-03-ui-frameworks-wasm.md))。

- **`alloc::borrow::Cow::Borrowed(..)`。** `alloc::` のパスを通した値も、`std::` や `core::` と同じように見つかる
  ようになった。
- **`Cow<'_, str>`。**
  - zustc の prelude には Cow が 2 つある: `Cow<[T]>` と、`Cow<str>` のための `__CowStr`。
  - 引数が `&str` か `String` の `Cow::Borrowed(s)` は、注釈がなくても str の方になった。型の推論でも、コード生成でも
    そうなる。前は推論がスライスの方を取り、`name` が `&[T]` になっていた: "no method `bytes` on type `str`"。
  - `Cow<str>` に対するパターン `Cow::Borrowed(s)` も str の方になる。
  - `Cow<str>` には `Display` と `Debug` があり、引数としては `&str` に型強制される。
- **`x.as_ref()`** は、スコープにある `AsRef` の境界に従う:
  - `T: AsRef<str>` なら `&str`、`T: AsRef<[u8]>` なら `&[u8]`;
  - クロージャの中 (`.map(|s| s.as_ref())`) でも同じで、その戻り値の型も推論されるようになった;
  - `&&str` に対しても働く。前は "cannot index into a value of type `str`" だった。
- **`$($t:ty)*`。** rustc と同じように、型は次の型が始まる所で終わるようになった:
  - `u8 u16 Vec<u8> (bool, char) fn(u8) -> u8 m!(u8)` は 6 つの型;
  - 丸括弧が型に付くのは、名前 (`Fn(u8)`) か型のマクロの `!` の後だけ。

  前は、繰り返しがリスト全体を 1 つの型として取っていた: wasm-bindgen の `vectors!`、num-traits の
  `int_trait_impl!`。
- **別の名前で呼ぶクレートやモジュール。** `use util as u;` や `extern crate util as u;` の後では、`u::f()` は
  `util::f()` になる。

## 2. 結果

- `crates/zurgo/tests/zurgo.rs`:
  - **`registry_crates_run_like_cargo`** に wasm-bindgen-shared 0.2.130 が入った。アプリはその公開 API を全部呼び、
    x86-64 で cargo のビルドと同じものを出す。そのビルドスクリプトは zustc がコンパイルする。
  - **`wasm32_runs_like_native`** (新しいテスト)。それぞれのフィクスチャを `--target wasm32-unknown-unknown` で
    ビルドして node で動かすと、x86-64 のビルドと同じものを出す。フィクスチャは:
    - パスのワークスペースと、ビルドスクリプトのパッケージ;
    - fnv、adler2、hex、itoa、rustc-hash、cfg-if、thiserror、wasm-bindgen-shared。

    違いは 2 つで、どちらもターゲットによるもの: ビルドスクリプトの `CARGO_CFG_*` の行と、cfg-if の `cfg(unix)` の
    腕。
- **`tests/wasm.rs` の `wasm_workspace_runs_like_native`** (新しいテスト)。tests/workspace (3 つのクレート) を
  1 つのモジュールにビルドする。どの経路でも x86-64 のビルドと同じものを出し、終了コードも同じ。経路には、別の
  クレートの総称関数の中の panic、`None` の `unwrap`、依存するクレートの中のゼロ除算がある。
- **`tests/probe/cow_asref_ty_frags.rs`** は rustc のビルドと同じものを出す。1.2 の穴をひととおり使う。
- **tests/workspace** は `use util as u;` を通して `util` を呼ぶ。そこでも rustc のビルドと同じである。
- **`cratebench/ui/run.py`** は wasm32 向けにビルドするようになり、それぞれのクレートに cargo の変数
  (`CARGO_PKG_*`) を渡す。ビルドスクリプトは、そこでは今も動かさない。同じ zustc で、x86-64 の cfg と変数なしの場合と
  比べた結果:

  | アプリ | 通るクレート | 前 |
  |---|---:|---:|
  | dioxus | 19 | 18 |
  | leptos | 18 | 17 |

  - wasm-bindgen-shared が通り、wasm-bindgen は `closures!` で止まる。
  - byteorder には 128 ビットのバイトの入れ替えが要った。
  - memchr の wasm32 の SIMD のモジュールは、`impl Vector for v128` で止まる。x86-64 のモジュールも、同じ種類の
    `__m128i` に対する impl で止まる。

wasm-bindgen-shared のアプリのコールドのビルド (3 回の中央値、4 コア)。どのビルドにも、ビルドスクリプトと
unicode-ident が入る:

| ビルド | 時間 | 出力 |
|---|---:|---|
| zurgo、x86-64 | 0.19 s | 59 KB の実行ファイル |
| zurgo、wasm32 | 0.14 s | 164 KB のモジュール |
| cargo、x86-64 (debug) | 0.59 s | 5.4 MB の実行ファイル |

zurgo の 2 つのビルドでは、wasm32 の方が速い: システムのリンカを動かさない。

KPI は cratebench のそれぞれのクレートの no-op の再ビルドで、その命令数を `bench/ci_bench.py` と同じように測る。
全部のクレートで、main は 101.02 億命令、このブランチは 100.57 億命令 (−0.4%)。1% より動いたクレートはない。

## 3. 次

mizchi/zust#38 の W1: wasm-bindgen そのもの。今わかっている穴は:

- `closures!` (tt-muncher) がどの呼び出しにもマッチしない;
- `ref_by_copy!` が `impl IntoWasmAbi for &&str` を作る。参照の参照に対する impl である;
- その proc macro (`#[wasm_bindgen]`、mizchi/zust#29) と ABI。

W0 のリストの残り (#38) は、それと一緒に進める:

- **extern の名前とは別の、クレートの同一性。** 1 つのクレートの 2 つの版と、Cargo.toml で名前を変えた依存
  (`package = ".."`) は、まだエラーになる;
- **クレートごとの edition;**
- **型の `stringify!`。** zustc はトークンごとに空白を入れる (`Vec < u8 >`) が、rustc は `Vec<u8>` と書く。
