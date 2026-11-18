# セルフホストの第 18 段階: codegen.rs を自前の構文木と minclif に移す (acg / driver2)

日付: 2026-11-16

- `src/acg/`：codegen.rs (syn + Cranelift、12,500 行) を、自前の構文木 (parse.rs) と minclif の上にメソッド単位でそのまま移したもの。
  `clif.rs` が Cranelift と同じ形の API (`b.ins().iadd(..)` など、i128 / 浮動小数点の振り分けつき) を minclif の上に作るので、
  IR を組み立てるコードはほぼ変えずに移せる。`mod.rs` が Cx の定義と中核 (メモリ・変数・関数本体・ブロック)、`p1`〜`p8` が残り
  (式 / 呼び出し / 演算 / メソッド / ループとマクロ / match と drop / 書式化・ハッシュ・比較 / その他)
- `src/driver2.rs`：syn と Cranelift を使わないドライバ。lexer / macros / prelude → ast_summary → Program → acg → minclif の ELF の .o → リンク。
  インスタンスの不動点・クロージャ・static・vtable・同じ機械語のインスタンスの畳み込み・panic の場所の表は driver.rs と同じ
- `--features acg` で有効になる。`ZUSTC_DRIVER2=1 zustc x.rs` でこちらを使う。`tests/driver2.rs` が probe / examples / acg を今のドライバと比べ、
  `tests/selfhost.sh` (当時は `SELFHOST_DRIVER2=1`。今は常にこの経路) がセルフホストの全段階 (lexer〜program・minclif・enc) を driver2 でビルドして rustc 版と比べる (すべて一致)
- 移して分かったこと:
  - minclif のブロックに後から戻って続きを書けなかった (Cranelift は書ける)。離れるときに続きへの jmp を置くようにした
  - i128 の引数・戻り値はレジスタ 2 つ (System V)。i128 の変数・ブロック引数も 16 byte で写す
  - コード生成は式を複製して書き換えるので、アドレスで引く型の記録が当たらない。syn 版はソース上の位置で引いていた。
    型推論が読んだマクロの引数の構文木をそのままコード生成に渡し、さらに「式の形 (Debug のハッシュ) → 型」の表で引く
  - 型推論が捨てた一時的な式のアドレスが、コード生成の新しい式に再利用されて違う型が当たっていた。推論の構文木はすべて最後まで持つ
  - 同じ文字列のクロージャ (`|x| x.1.clone()` が 2 か所) が同じ番号になっていた。本体のクロージャはアドレスで引く
  - 自前の構文解析の誤り (`&&x` のパターン、パターンの `true` / `false`)、整数の `clone()` の型推論の誤り (syn 版にもあった) も直した
