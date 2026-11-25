#!/usr/bin/env bash
# Runs the formal checks in formal/. `formal/run.sh tla` (seconds), `formal/run.sh connect` (a minute: the
# implementation against the models), `formal/run.sh kani` (minutes, needs
# `cargo install --locked kani-verifier && cargo kani setup`), `formal/run.sh lean` (the trait solver's proofs, needs
# Lean 4 (`lake`) on PATH, version in lean/lean-toolchain), no argument: all of them.
# Each check has an expected result: configurations that model a design zustc does not use must find its bug.
set -u
cd "$(dirname "$0")"
fail=0
log=$(mktemp) || exit 2
trap 'rm -f "$log"' EXIT
expect() { # expect <ok|violation> <spec.tla> <cfg>
    local flag=""; [ "$1" = violation ] && flag=--expect-violation
    if tla-check/target/release/zust-tla-check "tla/$2" "tla/$3" $flag > "$log" 2>&1; then
        echo "ok   $2 $3 ($1): $(head -1 "$log" | sed 's/^[^:]*: //')"
    else
        echo "FAIL $2 $3 (expected $1)"; cat "$log"; fail=1
    fi
}
if [ "${1:-all}" = all ] || [ "${1:-all}" = tla ]; then
    cargo build --release --quiet --manifest-path tla-check/Cargo.toml || exit 1
    expect ok        CacheSave.tla CacheSave.cfg
    expect violation CacheSave.tla CacheSaveShared.cfg
    expect ok        ForkPool.tla  ForkPool.cfg
    expect ok        ForkPool.tla  ForkPool3.cfg
    expect violation ForkPool.tla  ForkPoolCloseLate.cfg
    expect violation ArcDrop.tla   ArcDropPrelude.cfg
    expect ok        ArcDrop.tla   ArcDropAtomic.cfg
    expect ok        HashMapModel.tla HashMapModel.cfg
fi
if [ "${1:-all}" = all ] || [ "${1:-all}" = connect ]; then
    # (zustc's prelude, compiled by zustc, against the TLA+ models: the tests use target/release/zustc)
    (cd .. && cargo build --release --quiet) || exit 1
    cargo test --quiet --release --manifest-path connect/Cargo.toml || fail=1
fi
if [ "${1:-all}" = all ]; then
    (cd .. && cargo test --quiet -p zust-typeck --lib props) || fail=1
fi
if [ "${1:-all}" = all ] || [ "${1:-all}" = lean ]; then
    # (every theorem proved, on Lean's standard axioms only: the main theorems' axioms are propext, Classical.choice
    # and Quot.sound at most (a `sorry` would show as sorryAx, an axiom of the model's own by its name), and no
    # declaration of the library uses `sorry` (a lemma no main theorem uses only warns in the build))
    std_axioms() {
        grep -o "depends on axioms: \[[^]]*\]" "$1" | sed 's/.*\[//; s/\]//' | tr ',' '\n' | sed 's/^ *//; s/ *$//' | grep -v '^$' \
            | grep -vxE 'propext|Classical\.choice|Quot\.sound' | head -1 | grep -q . && return 1
        return 0
    }
    if (cd lean && lake build > "$log" 2>&1 && lake env lean Axioms.lean > "$log.ax" 2>&1) \
        && ! grep -q "declaration uses 'sorry'" "$log" && std_axioms "$log.ax"; then
        echo "ok   lean TraitSolver ($(grep -c "depends on axioms" "$log.ax") theorems)"
    else
        echo "FAIL lean TraitSolver"; cat "$log" "$log.ax" 2>/dev/null | tail -30; fail=1
    fi
    rm -f "$log.ax"
fi
if [ "${1:-all}" = all ] || [ "${1:-all}" = kani ]; then
    # (every harness is expected to pass)
    for dir in kani-prelude; do
        out=$( (cd "$dir" && cargo kani --output-format terse) 2>&1)
        rc=$?
        # (cargo kani exits nonzero when a harness fails, and when it cannot run at all: then there is no harness)
        if ! echo "$out" | grep -q "^Checking harness "; then
            echo "FAIL kani $dir: no harness was checked (exit $rc)"; echo "$out" | tail -20; fail=1
            continue
        fi
        if [ $rc -ne 0 ]; then
            echo "FAIL kani $dir: cargo kani exited with $rc"; echo "$out" | tail -20; fail=1
        fi
        while read -r h; do
            # (a harness is verified only when cargo kani finished without a failure: after a nonzero exit, one not
            # named as failed may have been interrupted)
            if echo "$out" | grep -q "Verification failed for - $h\$"; then r=failed
            elif [ $rc -ne 0 ]; then r=unknown
            else r=verified; fi
            if [ $r = verified ]; then echo "ok   kani $h ($r)"; else echo "FAIL kani $h ($r, expected verified)"; fail=1; fi
        done < <(echo "$out" | sed -n 's/^Checking harness \(.*\)\.\.\.$/\1/p')
    done
fi
exit $fail
