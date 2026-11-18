# セルフホストの第 16 段階: Cranelift の代わりのバックエンドの土台

日付: 2026-11-16

- `src/minclif.rs`：Cranelift の FunctionBuilder に寄せた API（ブロック・ブロック引数・変数・スタックスロット・整数の命令・比較・select・
  読み書き・呼び出し (System V、7 個目以降はスタック)・return）で x86-64 の機械語を直接出し、ELF の再配置可能オブジェクトを書く最小のバックエンド。
  値はすべてフレームのスロットに置く単純な生成。`tests/minclif.rs` がループ・再帰・8 引数の関数を組み立てて cc でリンクして動かす。
  zustc でコンパイルした版も同じバイト列の .o を書き、それをリンクして動く（tests/selfhost.sh の minclif）
- 浮動小数点 (f32 / f64 の四則・sqrt・丸め・比較 (NaN の扱いも)・符号の操作・整数との変換 (Rust の `as` と同じく飽和))、
  i128 (加減・ビット演算・比較・シフトは命令列、乗除は libgcc の `__multi3` など)、オーバーフローつきの演算、clz / ctz / popcnt /
  bswap / 回転 / min / max / abs / umulhi、trapz / trapnz、関数ポインタの呼び出し、名前のアドレス (GOT 経由)、
  浮動小数点の引数・戻り値 (xmm)、.data とその中のポインタ (R_X86_64_64) まで。`tests/minclif.rs` の ext_ops が 69 個の結果を rustc の計算と比べる
