# 設計

## crate の構成 (Cargo ワークスペース)

層は base → syntax → sema → typeck → codegen → core (driver) の順。syntax・sema・typeck がコンパイラと LSP・解析で
共有する部分で、LSP は core を feature `build` なしで使い、バックエンド (codegen / minclif / Cranelift) に依存しない。

| crate | 中身 | 依存 |
|---|---|---|
| `crates/base` (`zust-base`) | fxhash (FxMap / FxSet)・key (キャッシュのキー)・prof (段ごとの時間) | なし |
| `crates/syntax` (`zust-syntax`) | front (前処理 → アイテムへの分割 (差分) → prelude を付ける、の流れと編集をまたぐ記憶 FrontMemo) / lexer / parse (自前の構文木) / cfg / macros / ctors / prep (前処理の差分) / srcmap / stub / types (要約の型)・types_enc / enc (直列化) / ast_types / ast_closure / ast_summary (アイテムの要約) / prelude (選択と prelude_lib.rs.in、build.rs) | base、proc-macro2 |
| `crates/sema` (`zust-sema`) | program (名前・型・トレイト・impl の表、インスタンス) / solver / ast_infer (型推論) / ast_consteval | syntax |
| `crates/ir` (`zust-ir`) | 命令の列の型 (Type / Value / Block / Op …) と、命令を記録するだけの Recorder (値の型を覚える) | なし |
| `crates/typeck` (`zust-typeck`) | acg (構文木 → 命令の列の記録。型の検査はここで出る) / cache2 (Blob の直列化) | sema、ir、proc-macro2 |
| `crates/codegen` (`zust-codegen`) | lower (記録を minclif に組み立て直して機械語にする。Cranelift なら zust-clif に渡す) / link | typeck、minclif |
| `crates/core` (`zust-core`) | driver2 (Session2: 解析・check・キャッシュ) と、feature `build` (既定) の driver2/build.rs (機械語・fork の並列化・オブジェクト・リンク・ワークスペース)。下の層を公開し直す | typeck、(build なら) codegen |
| `crates/ide` (`zust-ide`) | エディタ向けの解析: symbols (documentSymbol / workspace/symbol のシンボル) / navigate (定義・参照・hover・補完。字句の列と定義の表からの軽い型の推定)。コンパイラ本体からは使わない | syntax |
| `crates/analyzer` (`zust-analyzer`) | LSP サーバ (バイナリ `zust-analyzer`)。診断は driver2 の check、それ以外は ide。ワークスペースの本文はメモリの上限まで | core (build なし)、ide、serde_json |
| `crates/minclif` | Cranelift に似た API の小さな x86-64 バックエンド (レジスタ割り当て・スタックの struct のフィールドの変数化・ELF の .o)。`from_recording` で記録を組み立て直す | ir |
| `crates/clif` (`zust-clif`) | driver2 の Cranelift バックエンド: minclif が組み立てた命令の列を Cranelift の IR にしてコンパイルする | minclif、cranelift |
| `.` (`zustc`) | CLI と、上のものの再輸出 (テスト・ベンチ)。ide と lsp には依存しない (ベンチの lsp/* だけが dev-dependency で使う) | 上のもの (ide・lsp を除く) |

各 crate の lib.rs は下の層のモジュールを公開し直す (`pub use zust_syntax::*;` など)。ファイルの中は `crate::lexer` の
ように書き、どの crate に置いても、セルフホストで全部を 1 つのクレートのモジュールに並べても同じパスで引ける。

型の検査とバックエンドの分け方: acg は命令を zust_ir::Recorder に記録するだけ (値・ブロックなどに番号を振り、値の型を
minclif と同じ決め方で覚える)。check はそこで止め、ビルドは codegen の lower が記録を minclif の FunctionBuilder に同じ順で
組み立て直してから機械語にする (minclif は命令の中で値やスロットを作るので番号は組み立て直す側で振り直す。
`ZUSTC_IR_VERIFY=1` で値の型が記録と同じかを確かめる)。出力の機械語は分ける前とバイト単位で同じ。

経路は **driver2** だけ (`zustc x.rs` は Cranelift、`--backend=minclif` で minclif)。build / check / LSP のすべてが driver2 を使う。
最初のドライバ (syn + Cranelift、`crates/legacy`) は 2026-11-16 に削除した (git の履歴にある)。
セルフホスト (tests/selfhost*.sh) は base / syntax / sema / ir / typeck / codegen / core と minclif のソースを 1 ファイルにまとめて zustc でコンパイルする。

## 処理の流れ (driver2)

今の主経路 (driver2):

```
src.rs ──srcmap (mod x; のファイル)──▶ macros (macro_rules!) ──▶ モジュールを平らに ──▶ lexer: アイテム + ハッシュ
   ──▶ 要約 (キャッシュ、fork で並列) ──▶ Program (シグネチャ・レイアウト) ──▶ 到達する関数
   ──▶ 関数ごと: キャッシュ? ── miss ──▶ ast_infer (型推論) → acg (自前の構文木から minclif の命令の列。-O で小さい関数を展開)
                                        → Cranelift (zust-clif) か minclif (レジスタ割り当て) で機械語 → Blob (fork で並列)
   ──▶ minclif の ELF 書き出しで .o (panic の場所の表を含む) ──▶ wild でリンク (変わらなければしない)
```

| 場所 | 役割 |
|---|---|
| `crates/syntax/src/lexer.rs` | トップレベルのアイテム分割と、正規化トークン列のハッシュ。`impl` はヘッダとメンバに分ける |
| `crates/syntax/src/macros.rs` / `srcmap.rs` | macro_rules! の展開・include_str!、`mod x;` のファイルの読み込みと行の対応 |
| `crates/syntax/src/parse.rs` / `ast_*.rs` | 自前の構文木・要約・型推論・クロージャ解析・定数評価 |
| `crates/sema/src/program.rs` | 全体の表 (シグネチャ・レイアウト・trait 実装・インスタンス) |
| `crates/typeck/src/acg/` | 構文木から命令の列 (zust_ir::Recorder) への 1 パスの lowering と型検査。-O で小さい関数をインライン展開 |
| `crates/core/src/driver2.rs` / `cache2.rs` | `Session2`：キャッシュ (ディスク + メモリ)、build / check / ワークスペース、fork の並列化、.o の書き出し |
| `crates/codegen/src/link.rs` | `cc -###` からリンク行を取り出して保存し、リンカを直接起動する |
| `crates/minclif` | 命令の記録・レジスタ割り当て・x86-64 の機械語・ELF の .o |
| `crates/clif` | 記録した命令の列 → Cranelift の IR → 機械語 |
| `crates/analyzer/src/lib.rs` | LSP サーバ (ドキュメントごとに `Session2` を持ち `check` の診断を返す。ワークスペースと std の索引) |
| `crates/ide/src/navigate.rs` / `symbols.rs` | 定義・参照・hover・補完の名前解決と、シンボルの一覧 |
| `src/main.rs` | CLI (build / check / ワークスペースの build) |

### インクリメンタル性の要点

（単相化について）
- ジェネリック関数とデフォルトメソッドは、要求された（関数, 型引数）ごとにインスタンスになる。
  キーは「元の関数のキー + 型引数（struct は deep hash）」。
- インスタンスは作業リストで不動点まで処理する。キャッシュした呼び出し元の Blob には、インスタンス参照を（元の関数名, 型引数）で保存してある。そのため読み戻すと、必要なインスタンスがまた要求される。
- 実測（examples/traits.rs、25 関数 / 11 インスタンス）：
  - `total_area<T>` の本体を編集すると、再コンパイルはその 2 インスタンスだけ。
  - デフォルトメソッド `score` の本体を編集すると、再コンパイルはそれを使う 2 型分だけ（上書きしている Rect は対象外）。
- check（LSP）でも、検査済みの印に「その関数が要求したインスタンス」を入れてある。
  そのため、呼び出し元がキャッシュに当たっても、ジェネリック本体の型エラーは報告される。

1. **callee の本体を変えても caller は再コンパイルされない。**
   キーに入るのは参照先のシグネチャだけで、Blob は callee を名前で参照している（blaze の template と同じ考え方）。
   シグネチャを変えた場合だけ、それを参照している関数が無効になる。
2. **struct は deep hash で追跡する。**
   - deep hash は、その struct 自身と、フィールドから推移的に届く全 struct の定義ハッシュをまとめたもの。参照フィールド経由も含む。
   - シグネチャのハッシュにも、struct は deep hash で入る。
   - そのため、`fn get(o: &Outer) -> i64 { o.inner.b }` のように `Inner` を名前で書いていない関数でも、`Inner` のレイアウトが変われば正しく再コンパイルされる（確認済み）。
3. **`impl` ブロックはメソッド単位で追跡する。**
   - lexer が `impl X {` のヘッダとメンバを別のアイテムに分ける。メンバのハッシュにはヘッダのハッシュを混ぜる。
   - そのため、メソッドを 1 つ編集しても parse し直すのはそのメソッドだけで、再コンパイルもそのメソッドだけ（確認済み：18 関数中 1 miss）。
   - シグネチャを変えた場合は、そのメソッドと呼び出し元だけが再コンパイルされる（2 miss）。
   - `x.len()` のようなメソッド呼び出しは、型を見ずに名前の最後のセグメント（`len`）で参照グラフに入れる。どの型の `len` も参照したことになるので過大近似だが、安全側になる。
4. **整形やコメント編集では何も再コンパイルされない。**
   キャッシュは全ヒットし、`.o` も同一になるのでリンクも省略される。
5. **変わっていないアイテムは parse もしない。**
   ファイル全体を syn で parse すると、1000 関数（320KB）で 80ms かかる。自前 lexer なら 2.6ms で済む。
6. **到達しない関数は parse も型検査もコード生成もしない。**
   その結果、オブジェクトにも入らない。
   その副作用として、到達しない関数の中の型エラーは報告されない（rustc なら報告される）。
7. **ディスク消費に上限がある。**
   保存するのは今回のビルドで使ったエントリだけなので、古い世代は自動的に消える。キャッシュのサイズは「今のコード量」程度に収まる。
8. **メモリ**
   - 同時に生きている AST は常に 1 関数分だけ。
   - Cranelift の `Context` と `FunctionBuilderContext` はスレッドごとに使い回す。
   - インクリメンタル時はスレッドを起動しない。
   - cold 時（要約のミスが 33 件以上）は、要約用の parse を並列にし、AST は持ち帰らない。
     全部保持すると、5000 関数では syn の AST だけでピーク RSS が 162MB になっていた。持ち帰らない形にして 35MB になった。

## 値の表現と ABI（内部用）

- **スカラー**（整数・bool・参照）は SSA 値で表す。参照はポインタ。
- **ローカル変数**
  - アドレスを取られないスカラーは Cranelift の `Variable`（レジスタ）に置く。
  - `&x` で参照されるものと struct はスタックスロットに置く。
- **struct**
  - 値は常にメモリ上にあり、式の評価結果は「そのアドレス」になる。
  - let・代入・引数・return の時点でコピーする（サイズは静的に分かるので、load/store に展開する）。
  - 引数：呼び出し側が一時領域にコピーし、そのアドレスを渡す。
  - 戻り値：呼び出し側が確保した領域のアドレスを、隠れ第 1 引数（sret）で渡す。
  - extern "C" 関数との間で struct を値で受け渡すことはできない（エラーになる）。
- **レイアウト**：宣言順に自然なアラインメントで並べる（repr(C) 相当）。
- **クロージャ**（`crates/syntax/src/ast_closure.rs`）
  - 要約の段階で、本体の k 番目（前順）のクロージャを次の 2 つに持ち上げる（lambda lifting）。
    - struct `親::{closure#k}<親の型引数, __C.., __P.., __R>`。フィールドはキャプチャで、参照キャプチャなら `*mut T`、`move` なら `T`。
    - fn `親::{closure#k}::call(__env: *mut その struct, 引数..) -> __R`。
  - キャプチャ・引数・戻り値の型がすべて struct の型引数に入る。そのためクロージャの型だけで本体のインスタンスが決まり、既存の単相化とインスタンス単位のキャッシュにそのまま乗る。
  - 本体は親のアイテムを parse し直し、k 番目を取り出して普通の fn にしてコンパイルする。キャプチャは `__env` のフィールドとしてスコープに入る。
  - 呼ばれないクロージャも、作った時点でインスタンスを要求する（本体のエラーを報告するため。未使用なら `--gc-sections` で消える）。
- **関数ポインタ**：関数のアドレス 1 語。`func_addr` で取り、`call_indirect` で呼ぶ（ABI は普通の呼び出しと同じ）。
- **`Box<T>`**：ヒープへのポインタ（`Box<dyn ..>` は fat pointer）。参照と同じ経路で自動 deref・dyn への変換をする。
- **drop**（`crates/typeck/src/acg/p6_match_drop.rs`）
  - drop が要る値を持つローカル変数・一時値は、1 byte の drop フラグと一緒に所有者として登録する。ムーブでフラグを 0 にし、スコープの出口でフラグが 1 のものを逆順に drop する（分岐でムーブするかどうかが変わっても正しい）。
  - drop 本体は prelude の `__exr_drop_in_place::<T>` のインスタンス（drop glue）。codegen が型ごとに 1 段だけ展開した本体を作り（`impl Drop` の呼び出し → フィールド / バリアント / 要素 → Box なら free）、入れ子の型は呼び出しにするので、再帰的な型でも展開が止まる。
  - vtable の最後の欄は中身の型の drop glue。`Box<dyn Trait>` の drop はここを呼んでから free する。
  - drop glue はインスタンス単位でキャッシュされる。`impl Drop` の有無は deep hash に入れてあり、後から足し引きしても、その型を使う関数が作り直される。
- **`&dyn Trait`**：(データのポインタ, vtable のアドレス) の 16 byte。
  - vtable は trait のメソッドの宣言順に並べた関数アドレスの列で、Blob に「関数 ID の列」として持つ。
  - キャッシュには関数名（インスタンスなら元の関数名と型引数）で保存する。
  - `.o` では内容で重複を除いた読み取り専用データにする。
  - `&dyn Fn(A) -> B` の vtable は、クロージャ本体のインスタンス 1 個だけ。
