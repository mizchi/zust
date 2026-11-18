# LSP の定義ジャンプと参照 (2026-11-17)

日付: 2026-11-17

`textDocument/definition` と `textDocument/references` に対応した (zust-syntax の `navigate`)。型推論はせず、字句の列と
アイテムの表 (symbols を平らにして、持ち主の型・モジュールのパスを付けたもの) で名前を解決する:

- ローカル: 関数の引数・`let` (`;` の後から見える)・`for` / `if let` / `while let` (次の `{` から)・match の腕・クロージャの引数。
- `x.name`: x の型 (`self` なら囲む impl、ローカルなら型注釈か `T::..` / `T { .. }` / `T(..)` の初期化) のメソッド・フィールド。
  型が分からないときは、同じ名前のメンバが一つだけのときだけ答える。
- `Q::name`: Q のメンバか、Q というモジュールの中のアイテム。`Self` は囲む impl の型。
- アイテム: 同じモジュール → `use` (名前・glob・glob で取り込んだモジュールの use、`as` の別名、`use` の中の名前そのもの) →
  ワークスペースで一つ → std。std の再エクスポート (`std::sync::Mutex` → sync::poison::mutex) は `pub use` をたどる。
  `#[derive(X)]` の X は derive のマクロ。組み込みの型は答えない。
- ワークスペース: initialize の root の Cargo.toml (members) から、各クレートの根 (lib / main / bin / tests / examples /
  benches / build.rs) と `mod x;` (`#[path]`・インラインの mod も) をたどってファイルのモジュールのパスを決める。tests などの
  クレートのアイテムは同じクレートからだけ見える。
- std / core / alloc (`rustc --print sysroot` の rust-src) は initialize の後に別スレッドで索引を作る (定義と use だけを持ち、
  本文は捨てて位置は行と桁で覚える: 約 7 MB)。`ZUST_ANALYZER_NO_STD=1` で切る。
- references は、同じ名前の識別子 (ファイルごとの名前の表で引く) のうち、definition が同じ所に解決されるもの。
  `macro_rules!` の本体の中は数えない (rust-analyzer と同じ)。
- メモリ: ワークスペースの全ファイルの字句の列 (u32 で持つ) と名前の表を持つので、zustc の対象で peak RSS 10.9 → 29.9 MB
  (std の索引を切ると 27.5 MB)。ready も 55 → 91 ms (ワークスペースの索引を作る)。

`python3 bench/lsp_parity.py` (rust-analyzer の答えと比べる。`--detail` で合わない所を出す):

## synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 |
| workspace/symbol (上位 20 の一致) | 87% | 100% | 76% |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 0% | 100% | 100% |
| references (数の一致) | 100% | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 0% (候補 0) | 100% (候補 3) | 100% (候補 5) |
| completion `na\|me` (同) | 0% (候補 0) | 91% (候補 145) | 56% (候補 5) |
| 時間の中央値 (ms) | defi 0.8, hove 0.2, comp 0.1, refe 5.8 | defi 0.4, hove 0.5, comp 2.5, refe 1.0 | defi 6.3, hove 6.3, comp 218.4, refe 18.8 |

## zustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (上位 20 の一致) | 73% | 100% | 35% |
| definition (行き先の一致) | 同じ 97% (答えた 80%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) |
| hover (名前を含む) | 0% | 88% | 72% |
| references (数の一致) | 64% | 100% | 42% |
| completion `x.\|` (書いてある名前を含む) | 0% (候補 0) | 100% (候補 26) | 90% (候補 37) |
| completion `na\|me` (同) | 0% (候補 0) | 68% (候補 85) | 60% (候補 3) |
| 時間の中央値 (ms) | defi 1.3, hove 0.3, comp 0.1, refe 13.6 | defi 0.8, hove 1.1, comp 6.6, refe 27.9 | defi 15.4, hove 13.0, comp 243.6, refe 315.0 |

- definition は synth で 100%、zustc で 97% (合わないのは、cfg で外れたアイテム `Backend` に答える所 (rust-analyzer は
  答えない) と、`FxMap::default()` を型の別名の先 (HashMap の impl) までたどれない所)。rust-glancer は 87%。時間は 1 回 1 ms ほど。
- references (数がちょうど同じ) は synth で 100%、zustc で 64% (rust-glancer 42%)。合わないのは主に型推論が要るもの:
  `Default::default()` の呼び出し先 (impl Default のメソッド)、`FxMap::default()` (型の別名をたどった先の std のメソッド)、
  `x.f()` の受け手の型がローカルの注釈から分からないもの。ほかは数が 1〜3 ずれるもの (`Ty` 2290 個など)。組み込みの型
  (`u8`) は rust-analyzer が参照を返すが zustc は返さない。時間は zustc の対象で 1 回 10〜14 ms (rust-analyzer 28 ms)。
- hover と completion は未対応。
