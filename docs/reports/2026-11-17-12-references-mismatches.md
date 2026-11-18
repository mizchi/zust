# references の残りの不一致を詰める (2026-11-17)

日付: 2026-11-17

`python3 bench/lsp_parity.py --detail` の合わない所を見て、数え方を rust-analyzer に合わせた。

- 型の参照に、その型の impl の中の `Self` も入れる (`impl Enc for Vec<T>` の `-> Option<Self>` は Vec の参照)。
- パターンと引数の束縛 (`let (a, concrete) = ..` / `fn f(concrete: T)` / `for (i, x) in` / `|a, b|`) はその束縛自身にする。
  前は、同じ名前の関数 `concrete` の参照に数えていた。
- `Q::name` の Q が知らない名前 (外部クレートの `syn::Pat`、`use proc_macro2::Delimiter` の `Delimiter::None`) なら答えない。
  前は `None` を Option の None に、`Pat` をワークスペースの Pat にしていた。
- cfg:
  - `#[cfg(..)]` で外れるアイテムは、定義から除き、中の名前も数えない。カーソルがその中にあれば答えない。
  - feature は Cargo.toml の `[features]` の default から、クレートごとにたどる。
  - ほかに分かるもの: test、debug_assertions、unix / windows、target_os と all / any / not。
- 組み込みの型 (`u8` など) の参照: ワークスペースの同じ名前の型の位置。`0u8` の接尾辞・`fn u8` の名前・`x.u8` は除く。

## synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 |
| workspace/symbol (上位 20 の一致) | 87% | 100% | 76% |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 100% | 98% | 100% |
| references (数の一致) | 100% | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 4) | 100% (候補 3) | 100% (候補 5) |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) | 56% (候補 5) |
| 時間の中央値 (ms) | defi 0.8, hove 0.7, comp 1.0, refe 3.7 | defi 0.5, hove 0.7, comp 3.7, refe 1.8 | defi 7.5, hove 7.6, comp 235.4, refe 19.8 |

## zustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (上位 20 の一致) | 73% | 100% | 35% |
| definition (行き先の一致) | 同じ 100% (答えた 78%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) |
| hover (名前を含む) | 82% | 88% | 72% |
| references (数の一致) | 78% | 100% | 39% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 40) | 100% (候補 26) | 90% (候補 37) |
| completion `na\|me` (同) | 80% (候補 7) | 68% (候補 85) | 60% (候補 3) |
| 時間の中央値 (ms) | defi 1.1, hove 1.0, comp 1.3, refe 16.6 | defi 0.9, hove 1.2, comp 6.2, refe 28.3 | defi 14.9, hove 12.5, comp 252.8, refe 330.1 |

- zustc の対象の references (数がちょうど同じ) は 67% → 78%。
- 残りの 9 か所は、どれも rust-analyzer 側の事情。
  - cfg で外れた `#[cfg(not(feature = "build"))] fn lower_blob` の中:
    - rust-analyzer は中の名前を数える (Vec・Analysis で 1〜2 個)。
    - 同じ cfg の `type Backend` には答えない。
  - `u8`: rust-analyzer は target/ の下に生成した prelude_items.rs (`include!`) の中も数える。
  - `Key`: build.rs の中の `mod key { pub type Key = u128; }` を、rust-analyzer は crates/base の Key の参照に数える。
- 時間: 参照は 1 回 17 ms (前は 9 ms。impl の中の `Self` も解決するため)。rust-analyzer は 28 ms。
- `ZUST_ANALYZER_MEM_MB=0` (全部 slim) でも同じ答え。
- hover は 85 → 82%: cfg で外れた `Backend` に答えなくなったため (rust-analyzer も答えない)。
