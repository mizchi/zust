# スタックの struct のフィールドをレジスタに置く (minclif)

日付: 2026-11-16

- スロット (struct・tuple・配列の一時領域) のアドレスを、解析のときに追う: `stack_addr` の値と、それに定数を足した値は
  「(スロット, オフセット)」。そのアドレスが load / store / 定数の足し算にしか使われず (呼び出しに渡す・保存する・
  ほかの計算に使うと逃げたとみなす)、読み書きが重ならず同じ場所はいつも同じ型なら、(スロット, オフセット) ごとに
  フィールドを 1 つの変数にする (scalar replacement)。割り当てでは値・変数と同じに扱い、レジスタに置けなかったフィールドは
  スロットのその場所を使う。アドレスの値は作らない
- acg の struct のコピーを、8 byte ずつからフィールドごと (その型) にした (読み書きの幅がフィールドと揃うように)
- 入口で 0 にするスロット (drop フラグ) は変数にしない。`MINCLIF_NO_SROA=1` で切れる
- 値で渡して値で返す struct のループ (tests/acg/struct_fields_regs.rs と同じもの、2000 万回): 176 ms → 147 ms (rustc -O 87 ms)。
  bench/rt はほとんど変わらない (nbody などの struct は Vec の中: ヒープ)
- セルフホスト版のコンパイラで自分自身: 9.7 秒

Rust を 1.100.0、wild を 0.11.0 に上げた (`rustup update stable`、`cargo install --locked wild-linker`)。
