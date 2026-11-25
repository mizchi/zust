#!/usr/bin/env bash
# Builds small programs using commonly used Rust features (tests/probe/*.rs) with rustc and zustc, compares the outputs,
# and prints how many passed. Watch the number grow as features are added (for the failures, the first line of the reason is printed too).
#   tests/probe.sh          all
#   tests/probe.sh NAME..   only the named ones
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q || exit 1
out=$(mktemp -d)
pass=0
total=0
names=("$@")
if [ ${#names[@]} -eq 0 ]; then
  for f in tests/probe/*.rs; do names+=("$(basename "$f" .rs)"); done
fi
for n in "${names[@]}"; do
  f=tests/probe/$n.rs
  total=$((total + 1))
  if ! rustc --edition 2024 -o "$out/$n.r" "$f" 2>/dev/null; then
    echo "SKIP $n (rustc がビルドできない)"
    continue
  fi
  # (if tests/probe/NAME.in exists, pass it as stdin)
  input=/dev/null
  [ -f "tests/probe/$n.in" ] && input="tests/probe/$n.in"
  "$out/$n.r" < "$input" > "$out/$n.r.out" 2>/dev/null
  rc_r=$?
  if ! err=$(target/release/zustc "$f" -o "$out/$n.e" 2>&1); then
    echo "FAIL $n: $(echo "$err" | head -1 | sed 's/^error: [^ ]*: //')"
    continue
  fi
  "$out/$n.e" < "$input" > "$out/$n.e.out" 2>/dev/null
  rc_e=$?
  if cmp -s "$out/$n.r.out" "$out/$n.e.out" && [ $rc_r -eq $rc_e ]; then
    pass=$((pass + 1))
  else
    echo "DIFF $n"
  fi
done
echo "probe: $pass / $total"
rm -rf "$out"
