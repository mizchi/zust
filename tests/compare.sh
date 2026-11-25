#!/usr/bin/env bash
# Builds examples/*.rs with both zustc and rustc and checks that the run results match.
# usage: tests/compare.sh   (run in this directory, after cargo build --release)
# (each example in a separate process, as many in parallel as there are cores; the output is printed together in the order of the examples)
set -u
self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$0")/.."
out=out/compare
mkdir -p "$out"
check() {
  local src=$1 name rflags c0 c1 c2
  name=$(basename "$src" .rs)
  # Check both without a cache (cold) and a rebuild from the cache (warm)
  rm -f "$out/$name.crcache2" "$out/$name.cold.crcache2"
  if ! ./target/release/zustc "$src" -o "$out/$name.cold" >"$out/$name.log" 2>&1; then
    echo "FAIL $name (zustc)"; cat "$out/$name.log"; return 1
  fi
  cp "$out/$name.cold.crcache2" "$out/$name.crcache2" 2>/dev/null
  if ! ./target/release/zustc "$src" -o "$out/$name" >"$out/$name.log" 2>&1; then
    echo "FAIL $name (zustc, warm)"; cat "$out/$name.log"; return 1
  fi
  # no_std (#![no_std] + #![no_main]) is built with libcore only, panic=abort, linking libc
  # (rust_eh_personality is not called with panic=abort, so define it as 0)
  if grep -q '^#!\[no_std\]' "$src"; then
    rflags=(--edition 2024 -C panic=abort -C link-arg=-lc -C link-arg=-Wl,--defsym=rust_eh_personality=0)
  else
    rflags=()
  fi
  rustc -Awarnings "${rflags[@]}" "$src" -o "$out/$name.rustc" || { echo "FAIL $name (rustc)"; return 1; }
  # Compare the exit codes too (for no_std, main's return value is the exit code)
  "$out/$name.cold" >"$out/$name.out0"; c0=$?
  "$out/$name" >"$out/$name.out1"; c1=$?
  echo "exit=$c0" >>"$out/$name.out0"
  if ! diff -q "$out/$name.out0" <("$out/$name"; echo "exit=$?") >/dev/null; then
    echo "FAIL $name (cold and warm builds differ)"; return 1
  fi
  "$out/$name.rustc" >"$out/$name.out2"; c2=$?
  echo "exit=$c1" >>"$out/$name.out1"; echo "exit=$c2" >>"$out/$name.out2"
  # If valgrind is available, also check for memory errors (double free, use after free, uninitialized reads) and leaks
  if command -v valgrind >/dev/null 2>&1; then
    # (the exit code is the program's; with -q, any output means a problem)
    valgrind -q --leak-check=full --errors-for-leak-kinds=definite --log-file="$out/$name.vg" \
        "$out/$name" >/dev/null 2>/dev/null
    if [ -s "$out/$name.vg" ]; then
      echo "FAIL $name (valgrind)"; head -20 "$out/$name.vg"; return 1
    fi
  fi
  if diff "$out/$name.out1" "$out/$name.out2" >"$out/$name.diff"; then
    echo "ok   $name"
  else
    echo "FAIL $name (output differs)"; cat "$out/$name.diff"; return 1
  fi
}

if [ "${1:-}" = "--one" ]; then
  name=$(basename "$2" .rs)
  check "$2" >"$out/$name.result" 2>&1
  exit $?
fi

printf '%s\n' examples/*.rs | xargs -P "$(nproc)" -n1 "$self" --one
fail=$?
for src in examples/*.rs; do
  cat "$out/$(basename "$src" .rs).result"
done
[ "$fail" -eq 0 ]
