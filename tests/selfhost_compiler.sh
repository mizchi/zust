#!/usr/bin/env bash
# Final self-host step: compiles the whole compiler without syn / Cranelift (combined into one file by tests/selfhost/build_compiler.py)
# with both rustc and zustc (session), compiles tests/probe / examples / tests/acg with the 2 resulting compilers, and checks
#   - whether the .o files the 2 compilers output are the same bytes
#   - whether the outputs of running them are the same
# Further, the zustc-built compiler compiles itself (compiler_main.rs) once more (stage 2),
# and it is checked that this gives the same .o as stage 1 (fixed point).
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q || exit 1
out=out/selfhost
mkdir -p "$out/cc/r" "$out/cc/c"
python3 tests/selfhost/build_compiler.py > "$out/compiler_main.rs"
rustc --edition 2024 -O -Awarnings "$out/compiler_main.rs" -o "$out/compiler_rustc" || exit 1
# (Stage 1 is minclif: it should match the result of the built compiler (minclif inside too) compiling itself)
if ! target/release/zustc --backend=minclif "$out/compiler_main.rs" -o "$out/compiler_zustc" -O; then
  echo "FAIL compiler (zustc)"; exit 1
fi
same=0; diff_o=0; diff_run=0; fail=0
for f in tests/probe/*.rs examples/*.rs tests/acg/*.rs; do
  n=$(basename "$f" .rs)
  a="$out/cc/r/$n"; b="$out/cc/c/$n"
  ra=0; rb=0
  "$out/compiler_rustc" "$f" -o "$a" 2> "$a.err" || ra=1
  "$out/compiler_zustc" "$f" -o "$b" 2> "$b.err" || rb=1
  if [ $ra -ne 0 ] || [ $rb -ne 0 ]; then
    if [ $ra -ne $rb ] || ! cmp -s "$a.err" "$b.err"; then
      echo "DIFF $n: build status differs"; fail=1
    fi
    continue
  fi
  if ! cmp -s "$a.o" "$b.o"; then
    echo "DIFF $n: .o differs"; diff_o=$((diff_o + 1)); fail=1
    continue
  fi
  oa=$("$a" < /dev/null 2>/dev/null; echo "exit $?")
  ob=$("$b" < /dev/null 2>/dev/null; echo "exit $?")
  if [ "$oa" != "$ob" ]; then
    echo "DIFF $n: output differs"; diff_run=$((diff_run + 1)); fail=1
    continue
  fi
  same=$((same + 1))
done
echo "compiled by both compilers: $same same, $diff_o .o differ, $diff_run outputs differ"
# Stage 2: compile itself with the zustc-built compiler
if "$out/compiler_zustc" "$out/compiler_main.rs" -o "$out/compiler_zustc2" -O; then
  if cmp -s "$out/compiler_zustc.o" "$out/compiler_zustc2.o"; then
    echo "ok   stage 2: the zustc-built compiler reproduces itself ($(wc -c < "$out/compiler_zustc2.o") bytes .o)"
  else
    echo "DIFF stage 2: .o differs from stage 1"; fail=1
  fi
else
  echo "FAIL stage 2 (compiler_zustc could not compile itself)"; fail=1
fi
# Whether the compiler built with the Cranelift backend also outputs the same .o
if target/release/zustc "$out/compiler_main.rs" -o "$out/compiler_clif" -O && "$out/compiler_clif" "$out/compiler_main.rs" -o "$out/compiler_clif2" -O; then
  if cmp -s "$out/compiler_zustc.o" "$out/compiler_clif2.o"; then
    echo "ok   the Cranelift-built compiler produces the same .o"
  else
    echo "DIFF the Cranelift-built compiler: .o differs"; fail=1
  fi
else
  echo "FAIL the Cranelift-built compiler"; fail=1
fi
exit $fail
