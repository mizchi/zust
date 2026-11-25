#!/usr/bin/env bash
# Regression check.
#   bench/regress.sh save    saves the current state as the baseline "main" (before adding features)
#   bench/regress.sh         compares with the baseline. Fails if there is anything criterion judges a "regression",
#                            a size budget (bench/budget.txt) overrun, or an output difference from rustc
# The linker can be fixed with the environment variable ZUSTC_BENCH_LINKER (compare under the same conditions as the baseline).
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q || exit 1
case "${1:-check}" in
  save)
    cargo bench --bench compile -- --save-baseline main 2>&1 | grep -E "time:|^[a-z]+/" ;;
  check)
    log=$(mktemp)
    cargo bench --bench compile -- --baseline main 2>&1 | tee "$log" | grep -E "time:|change:|regressed|improved|^[a-z]+/"
    fail=0
    if grep -q "Performance has regressed" "$log"; then
      echo "== 速度の回帰あり:"; grep -B3 "Performance has regressed" "$log" | grep -E "^[a-z]+/" ; fail=1
    fi
    cargo test --release -q --test budget 2>&1 | tail -3 || fail=1
    tests/compare.sh || fail=1
    rm -f "$log"
    [ $fail = 0 ] && echo "== 回帰なし"
    exit $fail ;;
  *) echo "usage: bench/regress.sh [save|check]"; exit 2 ;;
esac
