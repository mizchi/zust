# LSP のシンボル検索 (2026-11-17)

日付: 2026-11-17

`textDocument/documentSymbol` (ファイルのアウトライン) と `workspace/symbol` (名前で探す) に対応した。

- シンボルは zust-syntax の `symbols` で、字句の列から読む (構文木を作らない)。編集の途中で構文が壊れていても、読める所
  までは出す (本体の `{` は波括弧だけで対応を取り、閉じていなければ次の字下げなしのアイテムの前で切る)。位置は
  エディタのテキストそのもの (前処理の前) の UTF-16 の桁。
- 出すもの: mod / struct / enum / union / trait / impl (`impl Tr for S<T>` の名前) / fn (impl・trait の中は Method) / const /
  static / type / macro_rules!、struct とバリアントのフィールド、enum のバリアント、extern ブロックの中の宣言。detail に
  fn のシグネチャやフィールドの型。関数の本体の中は読まない。
- workspace/symbol は、開いているドキュメントと、initialize の rootUri の下の .rs ファイル (隠しディレクトリ・target・out・
  node_modules を除く) から探す。開いていないファイルは更新時刻と大きさで覚え、変わったものだけ読み直す。
- 合い方の順: 同じ → 前方一致 → 語の頭からの部分一致 → 部分一致 → 文字の順に含む (`nrm` → `norm`)。大文字小文字は区別しない。
  256 個まで。containerName は囲む mod / impl / trait。
- 速さ (このディレクトリをワークスペースにして): 1 回目 76 ms (261 ファイルを読む)、2 回目から 3 ms。
  compiler_main.rs (2.1 MB) のシンボル 2854 個を読むのに 40 ms (結果はドキュメントの本文が変わるまで覚える)。
- tests/lsp.rs の symbols、crates/syntax/src/symbols.rs の単体テスト。
