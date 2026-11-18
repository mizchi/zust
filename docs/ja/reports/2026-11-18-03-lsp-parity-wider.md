# LSP の比べ方を広げ、残りの違いを詰める (2026-11-18)

日付: 2026-11-18

**ベンチマーク (`bench/lsp_parity.py`) を直した**:

- 対象を 4 つに: synth (生成した 40 モジュール)・fixture (ide のテストのワークスペース、新)・zustc (driver2.rs)・
  ide (navigate.rs、新: 字句の部品が多い別の書き方のファイル)。
- rust-analyzer の準備ができるまで待つ: 名前が引けた後も型の解析を続けているので、`$/progress` と
  `experimental/serverStatus` (quiescent) を見て、作業が終わるまで待つ (前は fixture で rust-analyzer の補完が 0% に見えていた)。
- 時間は p50 と p90。起動から名前が引けるまで・作業が終わるまでの時間と、メモリの最大 (サーバと子のプロセスの RSS)。
- 「一致の平均」(機能ごとの一致の平均) を 1 つの数に。
- `--json bench/lsp_parity.json` で zustc の数字を残し、`--compare` で前回との差 (一致は点、時間は ms) を列に足す。
- `--detail` で workspace/symbol・documentSymbol の違いも出す。`--detail-out` でファイルに。

**直したもの** (どれも rust-analyzer に小さいワークスペースで聞いて、振る舞いを確かめてから合わせた):

- **std の再エクスポートが入っていなかった**: `FileIndex::library` が字句の列を捨ててから `pub use` を集めていた。
  `use std::sync::Mu|` の補完、`std::collections::HashMap` などのパスの解決に効く。
- **文字列・文字のリテラルの hover**: `&'static str` と値 (rust-analyzer と同じ形)。
- **`include!` と OUT_DIR**: `include!("x.rs")` と `include!(concat!(env!("OUT_DIR"), "/x.rs"))` のファイルも読む
  (OUT_DIR は target の下のビルドスクリプトの出力のいちばん新しいもの)。
- **`Self`**: definition は impl の見出しの型の名前、references はそれと同じ impl の中の `Self` (rust-analyzer と同じ)。
- **関数の中のアイテム** (`fn walk` を `fn new` の中に書いたもの) と **`thread_local! { static X }`** の static を定義にする
  (thread_local の中のものは rust-analyzer と同じく documentSymbol には出さない)。
- **documentSymbol の種類**: `self` を取らない関連関数は Function、static は Constant、`extern "C" { .. }` はブロックの
  記号 (Namespace) の子に。
- **workspace/symbol**: rust-analyzer の決まりに合わせた。
  - まず型 (struct / enum / union / trait / type とクレートの名前) だけを探し、なければ全部 (フィールド・impl・trait の impl の
    中のメンバは除く) から。
  - 文字が順に出てくるもの。大文字を含む問い合わせは大文字と小文字を区別する。名前の (小文字の) 順に 128 個まで。
  - クレートの名前: パッケージのターゲット (lib / bin / tests / benches / examples / build.rs) と Cargo.lock の依存と std。
  - `use a::B as C` の別名と `pub use` の再エクスポートも。
- **`fn` などのキーワードの上**: definition はその名前に。`fn` の上の references / documentHighlight は関数の出口
  (`fn`・`return`・`?`・最後の式。最後の式が match / if なら各腕・各ブロックの最後の式)。
- **クロージャの引数の型**: `v.iter().filter(|d| ..)` / `opt.map(|x| ..)` / `fold(0, |acc, x| ..)` の引数に、受け手の要素の型
  (filter などは参照)。補完・hover・定義に効く。
- **rename**: `enum E { A, B }` の宣言を構造体の省略形と取り違えて `A: new` にしていた。書き換える所がなければ断る。

**速くしたもの**:

- references: 候補の識別子ごとに名前の候補を全部の定義 (std を含めて 10 万個以上) から探していた。問い合わせ 1 回の間の
  名前の索引 (`named`) にした。メンバの参照で `Q::name` の Q が持ち主でない具体的な型なら解決もしない。
  サーバの中で 2.5 ms → 0.5 ms (synth)。
- documentHighlight: ワークスペース全体の参照を探してから絞っていたのを、そのファイルだけ探すように (zustc で 14.9 → 1.2 ms)。
- hover / signatureHelp: std のファイルを読み直すとき、本文だけにした (索引は作らない)。
- 応答は 1 回の書き込みに (ヘッダと本文を分けると stdout の行ごとの flush で 3 回になっていた)。

結果 (`python3 bench/lsp_parity.py --compare`、zustc / rust-analyzer。一致の平均の差は今回の前との差):

| 対象 | 一致の平均 zustc | rust-analyzer | 差 | references p50 zustc / ra (ms) | 起動から名前まで / メモリ zustc | rust-analyzer |
|---|---:|---:|---:|---|---|---|
| synth | 99.7% | 99.1% | +1.3 点 | 0.7 / 0.7 | 13 ms / 17 MB | 2689 ms / 868 MB |
| fixture | 97.3% | 95.6% | +15.4 点 | 0.9 / 0.9 | 7 ms / 17 MB | 332 ms / 833 MB |
| zustc | 96.3% | 95.5% | +4.3 点 | 5.4 / 22.4 | 139 ms / 53 MB | 4932 ms / 2368 MB |
| ide | 96.0% | 97.5% | +8.4 点 | 3.6 / 2.3 | 170 ms / 53 MB | 5391 ms / 1874 MB |

- workspace/symbol は 4 つとも 100% (前は 62〜87%)、documentSymbol は種類まで 100%、documentHighlight は 100%。
- rust-analyzer の一致の平均が 100% でないのは、答えなかった所 (definition の「答えた」など) を数えているため。
- fixture の差が大きいのは、前の比べ方では rust-analyzer の解析が終わる前に聞いていた (補完が 0%) 分も入っている。
- ide の `x.|` の補完は -10 点に見えるが、作業の間に navigate.rs が変わって比べる位置が違う。
- 残りの違い:
  - rename の `Default::default` (rust-analyzer は依存のクレートの中まで書き換える。わざと断る) と、
    `use a::{Point as Pt}` の Point の上 (rust-analyzer は別名だけを変えて使う所を変えない。zustc は定義を変える)。
  - references の `Key`: build.rs と共有する lexer.rs の中の別の `Key` を rust-analyzer が数える。
  - 型が分からない受け手の補完 (`x.as_str()` など)・std のメソッドの signatureHelp (`map(` など)・数の型の
    メソッド (`wrapping_sub`: core ではマクロで作る)。
