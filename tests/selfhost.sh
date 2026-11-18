#!/usr/bin/env bash
# セルフホストの確認: zustc 自身のソースの一部 (外部クレートに依存しないもの) を zustc でコンパイルし、
# rustc でコンパイルしたものと同じ入力で同じ出力になるかを確かめる。
#   lexer: crates/syntax/src/lexer.rs と crates/base/src/key.rs の KeyHasher (SipHash) → zustc 自身のソースを字句解析してアイテムとハッシュを出す
#   prelude: crates/syntax/src/prelude.rs (prelude の項目を名前で選ぶ。lexer / fxhash / KeyHasher と、build.rs が作る表を含む)
#            → probe / examples をユーザのソースとして、選ばれた prelude の長さとハッシュを出す
#   link:  crates/codegen/src/link.rs (リンカを探して直接起動する) と crates/core/src/testgen.rs → C の .o をリンクして実行する (std::path / process::Command)
#   macros: crates/syntax/src/macros.rs (macro_rules! の展開と include_str!。proc_macro2 の代わりに tests/selfhost/mini_pm2.rs を付ける)
#           → probe / examples / src を展開して、結果の長さとハッシュを出す
#   parse: crates/syntax/src/parse.rs (syn を使わない構文解析) → probe / examples / src を解析して節の数とアイテムを出す
#   ast_closure: crates/syntax/src/parse.rs + crates/syntax/src/ast_closure.rs (自前の構文木の上のクロージャ解析) → 全関数のクロージャの自由変数
#   ast_types: crates/syntax/src/types.rs + crates/syntax/src/parse.rs + crates/syntax/src/ast_types.rs (型の表現・intern と、型の構文 → TySyn。front.rs から分けたもの)
#   ast_summary: crates/syntax/src/ast_summary.rs (front.rs の summarize の移植) + lexer / types / parse / ast_types / ast_closure → 全アイテムの要約
#   program: crates/sema/src/program.rs (全体の表: シグネチャ・レイアウト・trait 実装) + 要約までのモジュール + prelude
#            + ast_consteval / solver / ctors → probe / examples / src の Program を作り、関数・型 (サイズ・オフセット・深いハッシュ)・
#            impl・const とそのデータ・パスの検索・型推論の単一化の結果を出す
#   minclif: crates/minclif/src/lib.rs (Cranelift の代わりの最小のバックエンド) → 関数を組み立てた ELF の .o が rustc 版と同じバイト列か。
#            zustc 版が書いた .o を C の main とリンクして動かす
#   enc:   crates/syntax/src/enc.rs (キャッシュの直列化。マクロ・trait の静的呼び出し) と crates/base/src/fxhash.rs (FxHasher / FxMap)
set -uo pipefail
cd "$(dirname "$0")/.."
# zustc 側のバックエンドは SELFHOST_BACKEND (既定 minclif: 外部クレートを使わない経路。cranelift も可)
cargo build --release -q || exit 1
zustc_cmd() { target/release/zustc --backend="${SELFHOST_BACKEND:-minclif}" "$@"; }
out=out/selfhost
mkdir -p "$out"
fail=0
python3 tests/selfhost/build_lexer.py > "$out/lexer_main.rs"
rustc --edition 2024 -O -Awarnings "$out/lexer_main.rs" -o "$out/lexer_rustc" || exit 1
if ! zustc_cmd "$out/lexer_main.rs" -o "$out/lexer_zustc" -O; then
  echo "FAIL lexer (zustc)"; exit 1
fi
inputs=(crates/*/src/*.rs crates/syntax/src/prelude_lib.rs.in examples/*.rs tests/probe/*.rs)
"$out/lexer_rustc" "${inputs[@]}" > "$out/lexer.rustc.out"
"$out/lexer_zustc" "${inputs[@]}" > "$out/lexer.zustc.out"
if cmp -s "$out/lexer.rustc.out" "$out/lexer.zustc.out"; then
  echo "ok   lexer ($(wc -l < "$out/lexer.rustc.out") lines over ${#inputs[@]} files)"
else
  echo "DIFF lexer"; diff "$out/lexer.rustc.out" "$out/lexer.zustc.out" | head; fail=1
fi
python3 tests/selfhost/build_prelude.py > "$out/prelude_main.rs"
rustc --edition 2024 -O -Awarnings "$out/prelude_main.rs" -o "$out/prelude_rustc" || exit 1
pin=(tests/probe/*.rs examples/*.rs)
"$out/prelude_rustc" "${pin[@]}" > "$out/prelude.rustc.out"
for opt in "" -O; do
  if ! zustc_cmd "$out/prelude_main.rs" -o "$out/prelude_zustc" $opt; then
    echo "FAIL prelude $opt (zustc)"; fail=1; continue
  fi
  "$out/prelude_zustc" "${pin[@]}" > "$out/prelude.zustc.out"
  if cmp -s "$out/prelude.rustc.out" "$out/prelude.zustc.out"; then
    echo "ok   prelude $opt (${#pin[@]} files)"
  else
    echo "DIFF prelude $opt"; diff "$out/prelude.rustc.out" "$out/prelude.zustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_link.py > "$out/link_main.rs"
printf '#include <stdio.h>\nint main(void){puts("hello from obj");return 0;}\n' > "$out/hello.c"
cc -c "$out/hello.c" -o "$out/hello.o" || exit 1
rustc --edition 2024 -O -Awarnings "$out/link_main.rs" -o "$out/link_rustc" || exit 1
rm -f "$out/hello_x" "$out/hello_x.link"
"$out/link_rustc" "$out/hello.o" "$out/hello_x" > "$out/link.rustc.out"
for opt in "" -O; do
  if ! zustc_cmd "$out/link_main.rs" -o "$out/link_zustc" $opt; then
    echo "FAIL link $opt (zustc)"; fail=1; continue
  fi
  rm -f "$out/hello_x" "$out/hello_x.link"
  "$out/link_zustc" "$out/hello.o" "$out/hello_x" > "$out/link.zustc.out"
  if cmp -s "$out/link.rustc.out" "$out/link.zustc.out"; then
    echo "ok   link $opt ($(wc -l < "$out/link.rustc.out") lines)"
  else
    echo "DIFF link $opt"; diff "$out/link.rustc.out" "$out/link.zustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_macros.py > "$out/macros_main.rs"
rustc --edition 2024 -O -Awarnings "$out/macros_main.rs" -o "$out/macros_rustc" || exit 1
min=(tests/probe/*.rs examples/*.rs crates/*/src/*.rs)
"$out/macros_rustc" "${min[@]}" > "$out/macros.rustc.out"
for opt in "" -O; do
  if ! zustc_cmd "$out/macros_main.rs" -o "$out/macros_zustc" $opt; then
    echo "FAIL macros $opt (zustc)"; fail=1; continue
  fi
  "$out/macros_zustc" "${min[@]}" > "$out/macros.zustc.out"
  if cmp -s "$out/macros.rustc.out" "$out/macros.zustc.out"; then
    echo "ok   macros $opt (${#min[@]} files, $(grep -c expanded "$out/macros.rustc.out") expanded)"
  else
    echo "DIFF macros $opt"; diff "$out/macros.rustc.out" "$out/macros.zustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_parse.py > "$out/parse_main.rs"
rustc --edition 2024 -O -Awarnings "$out/parse_main.rs" -o "$out/parse_rustc" || exit 1
"$out/parse_rustc" "${min[@]}" > "$out/parse.rustc.out"
for opt in "" -O; do
  if ! zustc_cmd "$out/parse_main.rs" -o "$out/parse_zustc" $opt; then
    echo "FAIL parse $opt (zustc)"; fail=1; continue
  fi
  "$out/parse_zustc" "${min[@]}" > "$out/parse.zustc.out"
  if cmp -s "$out/parse.rustc.out" "$out/parse.zustc.out"; then
    echo "ok   parse $opt (${#min[@]} files, $(wc -l < "$out/parse.rustc.out") lines)"
  else
    echo "DIFF parse $opt"; diff "$out/parse.rustc.out" "$out/parse.zustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_ast_closure.py > "$out/ast_closure_main.rs"
rustc --edition 2024 -O -Awarnings "$out/ast_closure_main.rs" -o "$out/ast_closure_rustc" || exit 1
"$out/ast_closure_rustc" "${min[@]}" > "$out/ast_closure.rustc.out"
for opt in "" -O; do
  if ! zustc_cmd "$out/ast_closure_main.rs" -o "$out/ast_closure_zustc" $opt; then
    echo "FAIL ast_closure $opt (zustc)"; fail=1; continue
  fi
  "$out/ast_closure_zustc" "${min[@]}" > "$out/ast_closure.zustc.out"
  if cmp -s "$out/ast_closure.rustc.out" "$out/ast_closure.zustc.out"; then
    echo "ok   ast_closure $opt ($(grep -c ': .* closures$' "$out/ast_closure.rustc.out") files)"
  else
    echo "DIFF ast_closure $opt"; diff "$out/ast_closure.rustc.out" "$out/ast_closure.zustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_ast_types.py > "$out/ast_types_main.rs"
rustc --edition 2024 -O -Awarnings "$out/ast_types_main.rs" -o "$out/ast_types_rustc" || exit 1
"$out/ast_types_rustc" "${min[@]}" > "$out/ast_types.rustc.out"
for opt in "" -O; do
  if ! zustc_cmd "$out/ast_types_main.rs" -o "$out/ast_types_zustc" $opt; then
    echo "FAIL ast_types $opt (zustc)"; fail=1; continue
  fi
  "$out/ast_types_zustc" "${min[@]}" > "$out/ast_types.zustc.out"
  if cmp -s "$out/ast_types.rustc.out" "$out/ast_types.zustc.out"; then
    echo "ok   ast_types $opt ($(wc -l < "$out/ast_types.rustc.out") lines)"
  else
    echo "DIFF ast_types $opt"; diff "$out/ast_types.rustc.out" "$out/ast_types.zustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_ast_summary.py > "$out/ast_summary_main.rs"
rustc --edition 2024 -O -Awarnings "$out/ast_summary_main.rs" -o "$out/ast_summary_rustc" || exit 1
"$out/ast_summary_rustc" "${min[@]}" > "$out/ast_summary.rustc.out"
for opt in "" -O; do
  if ! zustc_cmd "$out/ast_summary_main.rs" -o "$out/ast_summary_zustc" $opt; then
    echo "FAIL ast_summary $opt (zustc)"; fail=1; continue
  fi
  "$out/ast_summary_zustc" "${min[@]}" > "$out/ast_summary.zustc.out"
  if cmp -s "$out/ast_summary.rustc.out" "$out/ast_summary.zustc.out"; then
    echo "ok   ast_summary $opt ($(wc -l < "$out/ast_summary.rustc.out") lines)"
  else
    echo "DIFF ast_summary $opt"; diff "$out/ast_summary.rustc.out" "$out/ast_summary.zustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_program.py > "$out/program_main.rs"
rustc --edition 2024 -O -Awarnings "$out/program_main.rs" -o "$out/program_rustc" || exit 1
pin=(examples/*.rs tests/probe/*.rs crates/*/src/*.rs)
"$out/program_rustc" "${pin[@]}" > "$out/program.rustc.out"
for opt in "" -O; do
  if ! zustc_cmd "$out/program_main.rs" -o "$out/program_zustc" $opt; then
    echo "FAIL program $opt (zustc)"; fail=1; continue
  fi
  "$out/program_zustc" "${pin[@]}" > "$out/program.zustc.out"
  if cmp -s "$out/program.rustc.out" "$out/program.zustc.out"; then
    echo "ok   program $opt ($(wc -l < "$out/program.rustc.out") lines)"
  else
    echo "DIFF program $opt"; diff "$out/program.rustc.out" "$out/program.zustc.out" | head; fail=1
  fi
done
python3 tests/selfhost/build_minclif.py > "$out/minclif_main.rs"
rustc --edition 2024 -O -Awarnings "$out/minclif_main.rs" -o "$out/minclif_rustc" || exit 1
"$out/minclif_rustc" "$out/minclif.rustc.o" "$out/minclif2.rustc.o" > "$out/minclif2.want"
printf 'long mc_ext(void);\nint main(){return (int)mc_ext();}\n' > "$out/minclif2_c_main.c"
printf '#include <stdio.h>\nlong mc_sum(long);\nint mc_fib(int);\nlong mc_misc(long,long,long,long,long,long,long,long);\nint main(){printf("%%ld %%d %%ld\\n", mc_sum(10), mc_fib(20), mc_misc(1,2,3,4,5,6,7,8));return 0;}\n' > "$out/minclif_c_main.c"
for opt in "" -O; do
  if ! zustc_cmd "$out/minclif_main.rs" -o "$out/minclif_zustc" $opt; then
    echo "FAIL minclif $opt (zustc)"; fail=1; continue
  fi
  "$out/minclif_zustc" "$out/minclif.zustc.o" "$out/minclif2.zustc.o" > "$out/minclif2.zustc.want"
  if cmp -s "$out/minclif.rustc.o" "$out/minclif.zustc.o" && cmp -s "$out/minclif2.rustc.o" "$out/minclif2.zustc.o" \
     && cmp -s "$out/minclif2.want" "$out/minclif2.zustc.want" \
     && cc "$out/minclif_c_main.c" "$out/minclif.zustc.o" -o "$out/minclif_run" && [ "$("$out/minclif_run")" = "45 6765 -1005" ] \
     && cc "$out/minclif2_c_main.c" "$out/minclif2.zustc.o" -o "$out/minclif2_run" && "$out/minclif2_run" | cmp -s - "$out/minclif2.want"; then
    echo "ok   minclif $opt ($(wc -c < "$out/minclif.rustc.o") + $(wc -c < "$out/minclif2.rustc.o") bytes, $(wc -l < "$out/minclif2.want") checks run)"
  else
    echo "DIFF minclif $opt"; fail=1
  fi
done
python3 tests/selfhost/build_enc.py > "$out/enc_main.rs"
rustc --edition 2024 -O -Awarnings "$out/enc_main.rs" -o "$out/enc_rustc" || exit 1
for opt in "" -O; do
  if ! zustc_cmd "$out/enc_main.rs" -o "$out/enc_zustc" $opt; then
    echo "FAIL enc $opt (zustc)"; fail=1; continue
  fi
  if cmp -s <("$out/enc_rustc") <("$out/enc_zustc"); then
    echo "ok   enc $opt ($("$out/enc_rustc" | wc -l) lines)"
  else
    echo "DIFF enc $opt"; diff <("$out/enc_rustc") <("$out/enc_zustc") | head; fail=1
  fi
done
exit $fail
