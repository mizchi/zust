#!/usr/bin/env bash
# Mutation testing (cargo-mutants). The config is in .cargo/mutants.toml, the build uses profile.mutants.
# There are about 18,000 mutants in all, and each takes tens of seconds to build and test, so run them over a narrowed scope:
#
#   bench/mutants.sh diff [BASE]      only the mutants on the lines of the diff from BASE (default origin/main) (to check a change)
#   bench/mutants.sh pkg NAME         the mutants of crate NAME (e.g. zust-ide, zust-base)
#   bench/mutants.sh file PATH...     the mutants of files (e.g. crates/ide/src/symbols.rs)
#   bench/mutants.sh list [...]       only the list of mutants (to see the count). The rest are cargo mutants arguments
#
# The packages used for testing are chosen from the crates being mutated:
#   only crates/ide / crates/analyzer → the tests of zust-ide and zust-analyzer (fast)
#   otherwise → the tests of the whole workspace (many tests of the compiler layers are in the tests/ of zustc (the root))
# What follows `--` is passed to cargo mutants as is (e.g. -- -j 2 --shard 1/8 -F 'match_score').
# The results are in mutants.out/ (missed.txt: the mutants the tests did not catch, caught.txt, timeout.txt, unviable.txt).
#
# Requires: cargo install cargo-mutants --locked
set -uo pipefail
cd "$(dirname "$0")/.."
command -v cargo-mutants >/dev/null || { echo "cargo-mutants がない: cargo install cargo-mutants --locked"; exit 2; }

mode="${1:-}"
shift || true
args=()
extra=()
while [ $# -gt 0 ]; do
  if [ "$1" = "--" ]; then shift; extra=("$@"); break; fi
  args+=("$1"); shift
done

# The list of files to mutate → the test package options
tests_for() {
  local only_ide=1
  for f in "$@"; do
    case "$f" in
      crates/ide/*|crates/analyzer/*) ;;
      *) only_ide=0 ;;
    esac
  done
  if [ $only_ide = 1 ] && [ $# -gt 0 ]; then
    echo "--test-package zust-ide --test-package zust-analyzer"
  else
    echo "--test-workspace true"
  fi
}

case "$mode" in
  diff)
    base="${args[0]:-origin/main}"
    d=$(mktemp)
    git diff "$base" -- . > "$d"
    mapfile -t files < <(git diff --name-only --relative "$base" -- . | grep '\.rs$')
    if [ ${#files[@]} = 0 ]; then echo "$base からの .rs の差分がない"; rm -f "$d"; exit 0; fi
    # shellcheck disable=SC2046
    cargo mutants --workspace --in-diff "$d" $(tests_for "${files[@]}") "${extra[@]}"
    st=$?; rm -f "$d"; exit $st ;;
  pkg)
    [ ${#args[@]} -ge 1 ] || { echo "usage: bench/mutants.sh pkg NAME"; exit 2; }
    case "${args[0]}" in
      zust-ide|zust-analyzer) t="--test-package zust-ide --test-package zust-analyzer" ;;
      *) t="--test-workspace true" ;;
    esac
    # shellcheck disable=SC2086
    cargo mutants -p "${args[0]}" $t "${extra[@]}" ;;
  file)
    [ ${#args[@]} -ge 1 ] || { echo "usage: bench/mutants.sh file PATH..."; exit 2; }
    fs=()
    for f in "${args[@]}"; do fs+=(-f "$f"); done
    # shellcheck disable=SC2046
    cargo mutants --workspace "${fs[@]}" $(tests_for "${args[@]}") "${extra[@]}" ;;
  list)
    cargo mutants --list --workspace "${args[@]}" "${extra[@]}" ;;
  *)
    sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
