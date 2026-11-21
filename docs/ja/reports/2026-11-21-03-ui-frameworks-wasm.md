# zustc で dioxus と leptos を wasm32 向けにビルドできるか

日付: 2026-11-21

目標は、Rust の UI ライブラリを zustc でブラウザ向けにビルドすることである。このレポートは、よく使われる 2 つ、
dioxus と leptos を今の zustc で確かめる: それぞれがビルドできるか、そしてどちらもできないので、何が足りないか。
確かめ方は繰り返せる: `python3 cratebench/ui/run.py` (新しく足した) は、それぞれのフレームワークの wasm32 向けの
クレートを、葉から順に zustc でビルドし、結果を出す。

## 1. 答え

rustc は `cratebench/ui/apps/` の 2 つのカウンタのアプリ (シグナル 1 つ、見出しとボタン) を
`wasm32-unknown-unknown` 向けにビルドする: dioxus 0.7.10 (`web`) は 38 秒、leptos 0.8.23 (`csr`) は約 50 秒
(キャッシュなし、debug)。zustc はどちらもビルドできない:

| | dioxus 0.7.10 | leptos 0.8.23 |
|---|---:|---:|
| wasm32 向けにビルドするクレート | 168 | 123 |
| そのディレクトリの `.rs` の行数 | 1.36 M | 1.01 M |
| そのうち web-sys (有効な feature の分だけ) | 28 k (106 feature) | 36 k (162 feature) |
| ビルドスクリプトのあるもの | 23 | 22 |
| ホストのクレート (proc macro と、それが使うもの) | 66 | 76 |
| そのうち proc-macro のクレート | 28 | 22 |
| zustc: 通る | 15 | 14 |
| zustc: 自分のコードで失敗する | 47 | 39 |
| zustc: 失敗する依存に止められる | 106 | 70 |

- 通るのは小さな葉 (cfg-if、itoa、fnv、byteorder、scopeguard、pin-project-lite、rustc-hash 2、xxhash-rust など)
  か、proc macro を再公開するだけのクレート (pin-project、derive_more、zerofrom) である。
- ライブラリが通るとは、総称でないコードがコンパイルできたということである。zustc は総称関数の本体を、実体化する
  所でだけ検査する。
- ブラウザへの道はすべて止まっている:
  - wasm-bindgen は unicode-ident に;
  - js-sys と web-sys は、futures のクレートにも;
  - dioxus-core、dioxus-web、reactive_graph、tachys と leptos。
- クレートのグラフは `cargo metadata --filter-platform wasm32-unknown-unknown` から取る。版はコミットした
  Cargo.lock のもので、feature は解決されたもの。アプリから proc macro を通らずに届くクレートを「wasm32 向けに
  ビルドする」ものとする。
- 各クレートは、自分と依存からなる zustc のワークスペースとしてビルドする。依存が失敗するクレートはビルドしない。
- `zustc build` はまだ wasm32 を受け付けないので、スクリプトは x86-64 の cfg でビルドする。ワークスペースで wasm32
  の cfg を使う手元の zustc でも、通るクレートと失敗するクレートの組は同じで、最初のエラーが 1 つ違うだけだった
  (same-file)。

## 2. 足りないもの、下から順に

はじめの 4 つは、個々のクレートの話ではない。どれも、1 つだけで両方のフレームワークを止める。

### 2.1 Cargo のグラフをビルドする

zustc は、手で書いた一覧 (`lib <名前> <根> [依存] features=..`) からワークスペースをビルドする。crates.io からの
グラフには、さらに次のものが要る:

- Cargo.toml と Cargo.lock が決めること: 版、feature、名前を変えた依存、ターゲットごとの依存 (`run.py` は
  `cargo metadata` から取る);
- 1 つのグラフの中の、1 つのクレートの 2 つの版。ワークスペースではクレートごとに名前が 1 つなので、thiserror の
  1 と 2 (両方のツリーにある)、rustc-hash の 1 と 2、const-serialize の 0.7 と 0.8 は両方は置けない;
- クレートごとの edition: どちらのツリーにも 2015、2018、2021、2024 が混じる;
- ビルドスクリプト。その出力は:
  - cfg。wasm-bindgen の `wbg_reference_types` や、serde、anyhow、num-traits のもの;
  - 環境変数 (`cargo:rustc-env`)。wasm-bindgen-shared の `SCHEMA_FILE_HASH` など;
  - 生成するコード (`include!(concat!(env!("OUT_DIR"), ..))`)。serde、serde_core、thiserror 2 にある;
- `env!` / `option_env!` ("macro `env!` is not supported") と、Cargo が設定する変数 (`CARGO_PKG_VERSION` など);
- wasm32 向けのワークスペースのビルド: `zustc build` は `--target wasm32-unknown-unknown` を断る。

このほとんどは Cargo から得られる:

- `cargo metadata` がグラフを出す;
- `cargo check --message-format=json` がビルドスクリプトを動かし、その cfg、変数、`OUT_DIR` を知らせる
  (`build-script-executed`);
- 同じコマンドが、proc-macro のクレートをホスト向けにビルドする (`compiler-artifact`)。

### 2.2 proc macro

どちらのフレームワークも proc macro を通して使うもので、その下の多くのクレートも同じである:

- それぞれのアプリの 14 行に、`#[component]` と `rsx!` / `view!` が要る;
- js-sys と web-sys は、すべての束縛を `#[wasm_bindgen]` で宣言する (ツリーの 18 個のクレートが使う);
- serde の derive、thiserror の `#[derive(Error)]`、pin-project、async-trait、tracing の `#[instrument]`、leptos の
  props を作る typed-builder。

2 つのツリーの 222 個のクレートのうち、約 83 個が proc macro の derive か属性を使う。

zustc は proc macro を動かせず、その扱いもそろっていない:

- 知らない derive はエラーになる ("cannot find derive macro");
- 知らない関数形式のマクロはエラーになる ("macro `rsx!` is not supported");
- 知らない属性は捨てられる。そのため `#[wasm_bindgen] extern "C" { .. }` はただの C の import に、
  `#[component] fn` はただの関数になってコンパイルされる。

動かす方法は 3 つある:

1. proc-macro のクレートを zustc でホスト向けにコンパイルして動かし、zustc 自身の `proc_macro` クレートを渡す。
   それには syn、quote、proc-macro2 などが要る: ホストのクレートは 66 個と 76 個 (394 k 行と 411 k 行) あり、
   wasm の側と同じ穴にかかる。
2. rustc にビルドさせ (cargo がすでにしている)、rust-analyzer の proc macro サーバを通して読み込む。ツールチェーンに
   入っている (ここでは `libexec/rust-analyzer-proc-macro-srv` がある)。そのインターフェースは内部のもので、変わり
   うる ("an IDE implementation detail")。また zustc を、入っている rustc に結び付ける。
3. zustc の前に rustc で展開する (`-Zunpretty=expanded`。`RUSTC_BOOTSTRAP=1` が要る)。出力は安定でない内部の名前
   (`::alloc::boxed::box_assume_init_into_vec_unsafe`、`#[prelude_import]`) を使い、展開のために、差分ビルドの
   たびに rustc が要ることになる。

2 つのアプリの (rustc による) 展開を見ると、ライブラリとは別に、生成されるコードが何を要るかが分かる:

- `rsx!`:
  - `OnceLock` を持つ static と、クロージャから作る `GlobalSignal` の static;
  - const_format を通した定数の評価: 長さが `const fn` の呼び出しである配列と、`*const [u8] as *const str`;
  - `Box::new([..])` からの `Box<[T]>` と、`format_args!`;
  - `Signal<i32>` に対する `move |_| count += 1` のようなクロージャ (`AddAssign`);
- `view!`:
  - 型引数に既定値のあるビルダー (`struct AppPropsBuilder<TypedBuilderFields = ()>`);
  - `.child(..)` / `.on(..)` をつなげて作る、`HtmlElement<..>` の入れ子のタプルが型である `-> impl IntoView`。
    これには、型引数を持つ trait と出力の関連型、タプルへの impl が要る。

### 2.3 async は消される

zustc は `async fn`、`async {}`、`.await` を受け付けるが、ただのコードとしてコンパイルする:

- `async fn` は呼ばれた時に本体を実行し、その値を返す;
- `async fn f() -> i32` に対して `let x: i32 = f();` が型検査を通る;
- 一度も poll されない future も実行される;
- `pin!` には対応していない;
- `Box::pin(async { .. })` で作った `Pin<Box<dyn Future>>` は値を持つ ("no method `as_mut` on type `i32`")。

どちらのフレームワークも future の上で動く:

- dioxus の `spawn` と `use_resource`;
- leptos の `spawn_local`、`Resource`、`Suspense`;
- JS の promise と future をつなぐ wasm-bindgen-futures。

ツリーの 53 個のクレートが async を、48 個が `Waker` / `Context` / `Poll` を使う。async の本体を状態機械に変える
処理、`Pin`、タスクの型 (`Waker`、`RawWaker`、`Context`) が要る。

これは、zustc が今受け付けるプログラムの動きも変える: 副作用のある `async fn` は、それを違う時に起こす。
docs/language.md はそれを書いていなかったので、書いた。

### 2.4 wasm のモジュールと JavaScript の側

zustc の wasm のモジュールは、tests/wasm のサンドボックスのために作られている:

- ホストが与える `env` の関数だけを import する;
- `memory` と `main` だけを export し、`main` を最後まで実行する;
- ポインタと `usize` を 64 ビットのままにする (`target_pointer_width = "64"`)。

両方のツリーのすべての web の API が通る wasm-bindgen には、次のものが要る:

- 32 ビットの `usize` とポインタ: その ABI はポインタと長さを u32 で渡し (`WasmSlice { ptr: u32, len: u32 }`)、
  JS の glue もメモリをそのように読む;
- JS からの import と、`main` が返った後に JS が呼ぶ export の関数: イベントハンドラ、クロージャ、アロケータ、
  クロージャのためのテーブル;
- `#[wasm_bindgen]` が書くカスタムセクション `__wasm_bindgen_unstable`;
- `wasm-bindgen` CLI が JS の glue を作るためにインタプリタで動かす、`__wbindgen_describe_*` の関数。その形式は
  wasm-bindgen-shared の版に固定されている (`SCHEMA_VERSION`);
- wasm-bindgen のビルドスクリプトが有効にする externref (reference types)。

zust 自身の JS とのつなぎなら wasm-bindgen の形式は要らないが、dioxus と leptos は web-sys と js-sys の型を直接
使う。

### 2.5 言語

2 つのツリーのクレートの最初のエラーである。✓ は、同じ構文の小さなプログラムも zustc で失敗することを表す。

| 足りないもの | そこで止まるクレート | ✓ |
|---|---|:-:|
| 参照への参照への impl (`impl .. for &'a &'b str`、"unsupported impl self type"。wasm-bindgen の `ref_by_copy!` が `$t = &str` で `&$t` と書く) | wasm-bindgen (その変換) | ✓ |
| `impl Tr for &mut T` や `Box<S>` と並ぶ `impl Tr for &T` (E0119、conflicting implementations) | stable_deref_trait、tower-service | ✓ |
| self の型の違う impl にある同じ名前の固有メソッド (`Either<&L, &R>::cloned`、`Either<&mut L, &mut R>::cloned`) | either | ✓ |
| Send / Sync 以外の trait を含む `dyn` (`dyn Error + Send + Sync + UnwindSafe + 'a`) | thiserror 1 と 2 | ✓ |
| 型引数を持つ trait の関連型 (`impl<S> Layer<S> for Identity { type Service = S; }`) | tower-layer | ✓ |
| 型引数を持つ GAT (`type Output<Next> = (Next,);`) | next_tuple | ✓ |
| パスの隠れたライフタイムを通した省略 (`fn f(e: Ref<ErrorImpl>) -> Option<&Backtrace>`、E0106) | anyhow | ✓ |
| `Rc::new(S)` からの `Rc<dyn Tr>` | (dioxus-core は `Rc<dyn Any>` を持つ) | ✓ |
| `*const T` が要る所での `*mut T` と、生ポインタの `as_ref` | ryu、inventory | ✓ |
| 型引数の既定値 (`struct B<F = ()>`) | (`view!` の中の typed-builder の出力) | ✓ |
| 引数の型の中の `impl Trait` (`Option<&[impl AsRef<str>]>`) | wasm-bindgen-shared (3 節) | ✓ |
| クレートのパスを通した enum のヴァリアント (`alloc::borrow::Cow::Borrowed(..)`) と `Cow<'_, str>` | wasm-bindgen-shared (3 節) | ✓ |
| 定数の評価: 配列の長さとしてのループのある `const fn`、`const fn` で作る static の配列 | percent-encoding、httparse | ✓ |
| 整数のメソッドの連なりの中の `.bitxor(i)` (`bool` を期待する) | rustc-hash 1 | ✓ |
| `unsafe { a.get_unchecked(i) }` に対する整数のメソッド | unicode-ident | ✓ |
| 射影を通してだけ使う型引数が、使われていないと報告される | zmij | |

その構文を使うクレート (222 個のうち) の数が、重みの目安になる。正規表現で数えたものである:

- `&T`、`Box`、`Option`、スライスへの impl: 112;
- `-> impl Trait`: 59;
- `dyn A + B`: 51;
- GAT: 8。

### 2.6 macro_rules、モジュール、属性

- 断片 `ty` と `expr` は、次の区切りまでのトークンとして照合される。そのため区切りのない繰り返し `$($t:ty)*` は、
  一覧全体を 1 つの型として取る: num-traits (`int_trait_impl!(Num for usize u8 u16 ..)`)、wasm-bindgen
  (`vectors! { u8 i8 u16 .. }`)。✓
- もっと大きな tt-muncher は合わない:
  - pin-project-lite の `pin_project!` ✓。tokio、futures-util、event-listener、throw_error、http-body-util、tower が
    使う;
  - `bitflags!` ✓: その定義が読み込めさえしない ("unterminated item");
  - wasm-bindgen の `closures!` ✓;
  - konst の `__choose!`、bytes の `buf_try_get_impl!`、html-escape の `encode_impl!`、either_of の `tuples!`。
- 再帰の上限は 64 (rustc は 128) で、`#![recursion_limit]` を読まない: typenum、writeable。✓
- マクロの届く範囲:
  - `#[macro_use] extern crate x;` は x のマクロを持ち込まない (`x::m!` と `use x::m;` は持ち込む): encoding_rs
    (`cfg_if!`)、lock_api (`defer!`)。✓
  - 別のファイルの `#[macro_use] mod macros;` も持ち込まない (インラインの `mod` は持ち込む): httparse、zerocopy。✓
- `pub extern crate alloc;` は取り除かれない。取り除くのは `extern crate x;` だけ: smallvec。✓
- `#[cfg]` が取り除くアイテムの前の doc コメントが残り、エラーになる (E0585): enumset、matchit、litemap、
  wasm-bindgen。✓
- モジュール:
  - マクロが展開して作る `mod de;` を、マクロを呼んだ所ではなく、マクロを定義したファイルの隣で探す: serde_core の
    `crate_root!()`;
  - `#[cfg_attr(.., path = "..")] mod` をたどらない: mio。

### 2.7 prelude の std

次のものがなく、どれもどこかのクレートの最初のエラーである:

- `any::Any` (`downcast_ref` も)、`rc::Weak` と `sync::Weak`;
- `RwLock`、`LockResult` / `PoisonError`、`LazyLock`、`Once`、`thread::Thread`;
- `panic::AssertUnwindSafe` / `UnwindSafe`、`task::RawWaker`、`io::IoSlice` / `IoSliceMut`、`vec::Drain`;
- `str::FromStr`、`time::SystemTime`、`num::NonZero*`、`marker::PhantomPinned`、`path::Ancestors`;
- `CStr::from_bytes_until_nul`、`spin_loop_hint`、`ExactSizeIterator::len` の既定の実装。

また、prelude の `iter::Map` は型引数が 3 つで、std のものは 2 つなので、unicode-segmentation の `Map<I, F>` は
エラーになる。222 個のクレートのうち、それを使うものの数:

- `dyn Any`、`downcast`、`TypeId`: 28;
- `Weak`: 12;
- `RwLock`、`Mutex`、アトミック: 69;
- `thread_local!`: 33;
- `OnceLock`、`LazyLock`、`OnceCell`: 23。

## 3. どこまで深いか: wasm-bindgen を通る道

クレートの最初のエラーは、最初のものにすぎない。すべての web の API は unicode-ident → wasm-bindgen-shared →
wasm-bindgen を通る。この道で最初のエラーの先を見るため、これらのクレートの作業用のコピーを、エラー 1 つずつ
書き換えた:

| クレート | エラー | 書き換え |
|---|---|---|
| unicode-ident | `unsafe { .. get_unchecked(..) }` に対する `wrapping_shr` | `>>` |
| wasm-bindgen-shared | `Option<&[impl AsRef<str>]>` ("unsupported type") | 型引数 |
| | `alloc::borrow::Cow::Borrowed(..)` ("unsupported path") | `use` と `Cow::Borrowed` |
| | `Cow<'_, str>` ("no method `bytes` on type `str`") | `String` を返す |
| | `env!("CARGO_PKG_VERSION")`、`option_env!` | 定数 |
| wasm-bindgen | `closures!` がどの呼び出しにも合わない | そのモジュールを外す |
| | cfg で消えるアイテムの前の doc コメント (E0585) | doc コメントを消す |
| | `$($t:ty)*` | `$($t:ident)*` |
| | `ref_by_copy!` が `impl IntoWasmAbi for &&str` を作る ("unsupported impl self type") | (ここで止めた) |

wasm-bindgen-shared (670 行) には 5 つの書き換えが要った。wasm-bindgen (9.7 k 行) には、その proc macro と ABI が
関わる前に、少なくともあと 4 つある。その後に js-sys (23 k 行) と web-sys が来て、その束縛はすべて `#[wasm_bindgen]`
である。

## 4. 何が要るか

層を 1 つずつ取り除く順序。`cratebench/ui/run.py` で測れるのは 1 から 4 と 6、つまりクレートがビルドできるか
である。5 には wasm32 向けのビルドと ABI の確認 (モジュールに `wasm-bindgen` CLI をかける) が要り、それはしない。

1. 葉。`run.py` は失敗するクレートを、その上にあるクレートの数で並べる:
   - dioxus: serde_core 43、memchr 42、futures-core 38、unicode-ident 37、futures-sink 33、futures-io、
     futures-task、slab がそれぞれ 32;
   - leptos: memchr 29、unicode-ident 24、serde_core 23、futures-core 21。

   最初のエラーのほとんどは、2.7 の std の型、2.6 のマクロの穴、2.5 の impl の規則のどれかである。最初の目標:
   futures のクレート、slab、memchr、unicode-ident、serde_core。どれも proc macro も async も要らない。
2. Cargo を元にしたビルド (2.1): `cargo metadata` と、ビルドスクリプトと proc-macro の成果物のための
   `cargo check --message-format=json`; クレートごとの edition; 1 つのクレートの 2 つの版; `env!`。
3. async を状態機械に変える (2.3)。
4. proc macro (2.2 の 1 つ目か 2 つ目の方法)。
5. wasm-bindgen の ABI (2.4): wasm32 での 32 ビットのポインタ、import と export、カスタムセクション、reference
   types。そうすれば `wasm-bindgen` CLI が JS の glue を作れる。
6. フレームワーク:
   - dioxus-core: `Rc<dyn Any>`、generational-box;
   - reactive_graph: `Arc<RwLock<..>>`、`Weak`;
   - tachys: 型としてのビュー (タプル、関連型)。

2 から 5 はどれも大きな作業で、この 2 つのフレームワークにはそのすべてが要る。もっと小さな目標なら、早くブラウザに
届く: zustc のために作る UI ライブラリで、proc macro の代わりに macro_rules、自前の JS とのつなぎ、async の代わりに
コールバックを使うもの。

## 5. ファイル

- `cratebench/ui/apps/dioxus`、`cratebench/ui/apps/leptos`: カウンタのアプリ。版は Cargo.lock で固定してある。
- `cratebench/ui/run.py`: この確かめ (`--rustc` は先にアプリを cargo で wasm32 向けにもビルドする)。
- docs/language.md: async は消されると書いた。一時値の行を直した: [RefCell のレポート](2026-11-21-02-refcell-guards.md)
  から、`let` の一時値をすべてブロックの終わりまで残すことはしなくなった。
- TODO.md: UI フレームワークの節。ここを指す。
