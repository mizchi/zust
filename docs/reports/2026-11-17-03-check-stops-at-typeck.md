# check を型検査で止めて LSP を速くする (2026-11-17)

日付: 2026-11-17

- check (`zustc check` と LSP) は、acg のコード生成 (型の検査はここで出る) までにして、機械語 (Cranelift / minclif) を作らない
  (`Inline::check_only`)。検査済みの印は「要求した総称関数のインスタンスへの再配置だけを持つ空の Blob」で、ビルドの Blob と別の
  キー (`CHECK_SALT`) に覚える (ビルドの Blob があればそれも検査済みとして使う)。`ZUSTC_CHECK_CODEGEN=1` で機械語まで作る。
- prelude の選択 (`select_with`) がソースの全体を 80 回あまり `contains` で調べていたのを、アイテムごとに調べてアイテムの
  ハッシュで覚える形にした (`PreludeScan`)。それまで `SelectCache` を呼ぶたびに作り直していて、アイテムごとの記憶が効いて
  いなかったので Session2 に持たせた (`FrontMemo`)。
- `lexer::split` を差分にした (`split_inc`): 前回の入力と結果を覚え、前後の変わっていない部分のアイテムはそのまま (後ろは位置と
  行をずらして) 使い、間だけを split し直す。区切りは空白の後から始まるトップレベルのアイテム (impl / trait は丸ごと)。間の
  終わりがコメント・文字列の途中かもしれないときと、split が失敗するときは全体を split する。tests/split_inc.rs は実際のソースに
  乱数の編集を続けて加え、毎回全体の split と比べる。`ZUSTC_SPLIT_VERIFY=1` で毎回比べる、`ZUSTC_NO_SPLIT_INC=1` で切る。

| | 前 | 後 |
|---|---:|---:|
| `zustc check` compiler_main.rs (38886 行) | 3.6 s | 1.6 s |
| LSP didOpen / didChange、生成した 1000 関数 (17019 行、stdio 越し) | 135 / 12.2 ms | 122 / 9.6 ms |
| LSP didOpen / didChange、compiler_main.rs (1 関数の編集) | 2.09 s / 523 ms | 1.63 s / 476 ms |
| split (生成した 1000 関数の didChange) | 2.0 ms | 0.6 ms |

残り (compiler_main.rs の didChange 476 ms の内訳): ファイル全体を書き換える前処理 (マクロの展開 97 ms・`mod` の平らにする 63 ms・
入れ子のアイテムの移動 47 ms・コンストラクタの書き換え 33 ms。それぞれがファイル全体を字句解析し直す)、Program と参照 47 ms、
全関数 (9115) のキャッシュのキーと読み戻し 100 ms。どれも全体を作り直しているので、次はアイテムごとの差分にする。
