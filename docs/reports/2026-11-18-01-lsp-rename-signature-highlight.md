# LSP に rename / signatureHelp / documentHighlight を足す (2026-11-18)

日付: 2026-11-18

どれも、もとからある definition / references / シグネチャの部品の上に作った (`crates/ide/src/navigate.rs` の
`prepare_rename` / `rename` / `signature_help`、`crates/analyzer/src/lib.rs` のハンドラ)。

- **rename** (`prepareProvider` も):
  - references の位置を書き換える。trait のメソッドなら impl のメソッドも書き換える。
  - 構造体の省略形は形を変える: フィールドの名前を変えると `S { x }` → `S { new: x }`、ローカルの名前を変えると
    `S { x }` → `S { x: new }`。書式の取り込み `"{x}"` の中も書き換える。
  - `use a::B;` の B の上なら、別名を付ける (`use a::B as New;` と、そのファイルの中の B)。rust-analyzer と同じ形。
  - 断るもの (エラー `RequestFailed` に理由を付ける): キーワード・組み込みの型・ライブラリの定義 (use の名前の上の
    ときを除く)・ファイルのモジュール (`mod x;`、ファイルの名前も変わるので)・derive が作るメソッド・名前にできない文字列。
- **signatureHelp**: カーソルを囲む呼び出し `f(..)` / `x.m(..)` / `T::f(..)` / `f::<T>(..)` / タプル構造体 `T(..)`。
  引数の範囲は UTF-16 の桁。メソッドの呼び出しでは self を引数に数えない (rust-analyzer と同じ)。引数の中の配列や
  `vec![..]` の中でも外側の呼び出しを答える。宣言 `fn f(..)` の中では答えない。
- **documentHighlight**: 同じファイルの中の references。

rust-analyzer との比べ方を `bench/lsp_parity.py` に足した (rename は書き換えの集合が同じか、signatureHelp は
名前を含むか・引数の数・今の引数が同じか、documentHighlight は数):

| 対象 | 機能 | zustc | rust-analyzer |
|---|---|---|---|
| synth | rename | 100% | 100% |
| synth | signatureHelp | 100% (答えた 100%) | 100% (答えた 95%) |
| synth | documentHighlight | 100% | 100% |
| zustc | rename | 82% (断ったのも同じ 23/23) | 100% |
| zustc | signatureHelp | 89% (答えた 85%) | 100% (答えた 95%) |
| zustc | documentHighlight | 85% | 100% |

- 時間の中央値 (zustc を対象): rename 1.3 ms (rust-analyzer 0.9)、signatureHelp 1.3 ms (同 0.4)。
  ほかの機能の時間と一致は前と同じ (references 14.5 ms、rust-analyzer 26.4 ms)。
- 残りの違い:
  - rename・documentHighlight: rust-analyzer が数える cfg で外れたアイテムの中 (references と同じもの)。
    `Default::default` を rust-analyzer は依存のクレートの中まで 1,273 か所書き換える (zustc はワークスペースの 16 か所)。
  - signatureHelp: std の trait のメソッド (`extend`) と、クロージャの呼び出し (`body_hash(..)`) に答えない。
- 最初に測ったときは rename 71%・signatureHelp 26% だった。rust-analyzer と比べて直したもの:
  メソッドの呼び出しの self (引数の数と今の引数が 1 ずれていた)、use の名前の上の rename (別名にする)、
  パスで書いた `std::sync::Mutex` の上の rename は断る。
