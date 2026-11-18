# LSP の hover と補完、大きいワークスペースのメモリ (2026-11-17)

日付: 2026-11-17

`textDocument/hover` と `textDocument/completion` (`.` と `:` で起動) に対応した。どちらも定義・参照と同じ索引
(zust-syntax の `navigate`) から引く。型検査は使わない。

- hover: 定義のシグネチャ (属性を外し、本体の `{` の前まで)・囲むもの (impl / モジュール)・その前の `///` の doc コメント。
  ローカルは推定した型 (`q: Point`)、組み込みの型はその名前。
- 補完:
  - `x.|`: 受け手の型のフィールドとメソッド。
  - `Q::|`: 型のメンバとモジュールのアイテム。re-export した名前も入れる (`std::sync::Mu|`)。
  - それ以外: 見えるローカル・ワークスペースのアイテム・use した名前・prelude・よく使うマクロ・予約語。
  - 打った途中までで絞る: 前方一致・語の頭。ローカルは文字の順でも合わせる。
- 受け手の型の推定 (文字列で持つ):
  - たどれるもの: ローカルの型注釈・`let` の初期化の式・引数・`self`、フィールドの型、メソッドと関数の戻り値。
  - impl の型引数は受け手の引数で置き換える。例: `Option<Sig>` の `unwrap` → `Sig`、`HashMap<K, V>` の `get` → `Option<&V>`。
  - 式の形: 添字 (`Vec<T>[i]` → T、map → V)、`?`、`for x in v`、`if let Some(x) = ..`、`vec![]`、構造体の式、`as T`。
  - 型の別名は型引数ごと展開する (`LockResult<G>` → `Result<G, ..>`)。Deref は包みの中身を見る (`MutexGuard<T>` → T)。
  - impl / derive された trait のメソッド (`clone`、`Iterator` の `map` など) も見る。
  - `..Default::default()` と `field: Default::default()` は、構造体の式の型・フィールドの型の `default` にする。
- std の索引で直したところ:
  - `pub const unsafe fn` と `const trait` を読めていなかった (core の `impl str` のメソッドの大半が落ちていた)。
  - `sys`・テスト・ほかの OS 向けのファイルを入れない。
  - 位置は定義に、モジュールのパス・囲むものは共有して持つ: 約 21 MB → 10 MB。
- 大きいワークスペース (`ZUST_ANALYZER_MEM_MB`、既定 4):
  - ワークスペースの本文と字句の列は、合計がこの大きさまでのファイルの分だけ持つ。
  - 超えた分のファイルは slim にして、定義・`pub use`・出てくる名前のハッシュの集合だけを持つ。
  - 参照・hover で要るときは、名前のハッシュで選んだファイルだけ読み直す。読み直したものは同じ大きさまで LRU で覚える。
  - `ZUST_ANALYZER_MEM_MB=0` (全部 slim) でも答えは変わらない (parity は同じ)。参照は遅くなる: zustc の対象で 1 回 9 → 31 ms。

`python3 bench/lsp_parity.py`:

## synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 100% (種類も 97%) | 名前 100% (種類も 100%) | 名前 100% (種類も 95%); 余分 #0 |
| workspace/symbol (上位 20 の一致) | 87% | 100% | 76% |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) |
| hover (名前を含む) | 100% | 100% | 100% |
| references (数の一致) | 100% | 100% | 100% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 4) | 100% (候補 3) | 100% (候補 5) |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) | 56% (候補 5) |
| 時間の中央値 (ms) | defi 0.9, hove 0.8, comp 1.0, refe 3.5 | defi 0.4, hove 0.6, comp 3.1, refe 1.0 | defi 6.7, hove 6.7, comp 224.6, refe 18.9 |

## zustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (名前の一致) | 名前 99% (種類も 97%); 足りない extern "C" | 名前 100% (種類も 100%) | 名前 90% (種類も 82%); 足りない c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (上位 20 の一致) | 73% | 100% | 35% |
| definition (行き先の一致) | 同じ 100% (答えた 80%) | 同じ 100% (答えた 78%) | 同じ 87% (答えた 68%) |
| hover (名前を含む) | 85% | 88% | 72% |
| references (数の一致) | 67% | 100% | 39% |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 40) | 100% (候補 26) | 90% (候補 37) |
| completion `na\|me` (同) | 80% (候補 7) | 68% (候補 85) | 60% (候補 3) |
| 時間の中央値 (ms) | defi 1.0, hove 1.0, comp 1.3, refe 8.8 | defi 1.0, hove 1.1, comp 7.0, refe 30.9 | defi 16.0, hove 13.9, comp 265.5, refe 388.4 |

- zustc の対象で、hover 85% (rust-analyzer 88%) と `x.|` の補完 100% (候補の数の中央値 40、rust-analyzer 26)。
  rust-glancer はそれぞれ 72% と 90% (候補 37)。
- hover で合わないのは文字列の中の名前 (`"ZUSTC_INLINE_LOG"`) の 2 か所だけ。
- `na|me` は、rust-analyzer がほかの候補を多く出す (中央値 85 個) ため、そこに書いてある名前が入らないことがある (68%)。
  zustc は絞るので 80% (候補 7)。この指標で上回っても、補完の質が上とは言えない。
- definition は 100% になった (`FxMap::default()` を別名の先の HashMap の impl にした)。references は 67%。残りの多くは、
  1〜3 個ずれる数え方の違いと、組み込みの型 (`u8`) の参照。
- 時間 (1 回、zustc の対象): definition / hover 1.0 ms、補完 1.3 ms、参照 9 ms。rust-analyzer は 1.0 / 1.1 / 7.0 / 31 ms。

`python3 bench/lsp_compare.py --cold` (参照の列を足した。big はモジュール 1000 個・約 11 万行・4.5 MB の生成クレート):

| 対象 | サーバ | init | ready | docsym (数) | wsym (見つかった) | edit→docsym | edit→diag | references 1 回目 / 2 回目 (数) | peak RSS (子も) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| synth | zustc | 4.9 ms | 14.6 ms | 0.8 ms (37) | 0.9 ms (5/5) | 2.1 ms | 1.5 ms | 207 ms / 3.5 ms (29) | 16.9 MB (15.8 MB) |
| synth | rust-analyzer | 23.9 ms | 3398 ms | 1.4 ms (37) | 1.0 ms (5/5) | 19.7 ms | 299 ms | 84.0 ms / 3.2 ms (29) | 596 MB (865 MB) |
| synth | rust-glancer | 26.0 ms | 5812 ms | 1.9 ms (38) | 31.2 ms (5/5) | 4.7 ms | - | 380 ms / 14.6 ms (29) | 15.7 MB (888 MB) |
| zustc | zustc | 2.5 ms | 86.7 ms | 1.8 ms (123) | 1.2 ms (5/5) | 13.6 ms | 10.9 ms | 71.9 ms / 10.0 ms (36) | 30.1 MB (30.1 MB) |
| zustc | rust-analyzer | 25.7 ms | 6653 ms | 4.4 ms (124) | 1.1 ms (5/5) | 12.3 ms | - | 1698 ms / 15.1 ms (36) | 1571 MB (1633 MB) |
| zustc | rust-glancer | 106 ms | 11194 ms | 7.7 ms (113) | 265 ms (5/5) | 48.0 ms | - | 1702 ms / 122 ms (36) | 29.8 MB (1799 MB) |
| big | zustc | 2.4 ms | 152 ms | 0.8 ms (37) | 14.4 ms (5/5) | 3.7 ms | 3.1 ms | 47.6 ms / 16.6 ms (29) | 55.1 MB (55.1 MB) |
| big | rust-analyzer | 20.1 ms | 4293 ms | 5.7 ms (37) | 14.7 ms (5/5) | 5.2 ms | 4875 ms | 487 ms / 7.1 ms (29) | 825 MB (2038 MB) |
| big | rust-glancer | 35.5 ms | 6206 ms | 1.9 ms (38) | 108 ms (5/5) | 5.0 ms | - | 926 ms / 29.4 ms (29) | 22.0 MB (1033 MB) |

- 参照の 1 回目は std の索引 (initialize の後に別スレッドで作る) を待つ時間を含む: synth で 207 ms。
  rust-analyzer と rust-glancer の 1 回目 (0.5〜1.7 秒) よりは短い。
- peak RSS:
  - zustc の対象: 定義・参照を入れた直後の 29.9 MB から 30.1 MB (hover・補完のための型の情報・std の impl を足しても、
    std の索引を小さくした分で相殺)。
  - big: 55 MB (`ZUST_ANALYZER_MEM_MB=0` で 28 MB)。
  - rust-analyzer は 0.6〜1.6 GB。rust-glancer は本体 16〜30 MB だが、子プロセスを足すと 0.9〜1.8 GB。
