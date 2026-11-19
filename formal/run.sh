#!/usr/bin/env bash
# Runs the formal checks in formal/. `formal/run.sh tla` (seconds), `formal/run.sh kani` (minutes, needs
# `cargo install --locked kani-verifier && cargo kani setup`), no argument: both.
# Each check has an expected result: specs and harnesses named after a bug are expected to find it.
set -u
cd "$(dirname "$0")"
fail=0
expect() { # expect <ok|violation> <spec.tla> <cfg>
    local flag=""; [ "$1" = violation ] && flag=--expect-violation
    if tla-check/target/release/zust-tla-check "tla/$2" "tla/$3" $flag > /tmp/zust-tla.$$ 2>&1; then
        echo "ok   $2 $3 ($1): $(head -1 /tmp/zust-tla.$$ | sed 's/^[^:]*: //')"
    else
        echo "FAIL $2 $3 (expected $1)"; cat /tmp/zust-tla.$$; fail=1
    fi
    rm -f /tmp/zust-tla.$$
}
if [ "${1:-all}" != kani ]; then
    cargo build --release --quiet --manifest-path tla-check/Cargo.toml || exit 1
    expect violation CacheSave.tla CacheSave.cfg
    expect ok        CacheSave.tla CacheSaveFixed.cfg
    expect ok        ForkPool.tla  ForkPool.cfg
    expect ok        ForkPool.tla  ForkPool3.cfg
    expect violation ForkPool.tla  ForkPoolCloseLate.cfg
    expect violation ArcDrop.tla   ArcDropPrelude.cfg
    expect ok        ArcDrop.tla   ArcDropAtomic.cfg
fi
if [ "${1:-all}" = all ]; then
    (cd .. && cargo test --quiet -p zust-typeck --lib props) || fail=1
fi
if [ "${1:-all}" != tla ]; then
    # (bug_* harnesses are expected to fail; every other one to pass)
    for dir in kani-prelude; do
        out=$( (cd "$dir" && cargo kani --output-format terse) 2>&1)
        while read -r h; do
            if echo "$out" | grep -q "Verification failed for - $h\$"; then r=failed; else r=verified; fi
            case "$h" in *::bug_*) want=failed ;; *) want=verified ;; esac
            if [ $r = $want ]; then echo "ok   kani $h ($r)"; else echo "FAIL kani $h ($r, expected $want)"; fail=1; fi
        done < <(echo "$out" | sed -n 's/^Checking harness \(.*\)\.\.\.$/\1/p')
    done
fi
exit $fail
