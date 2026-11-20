#!/usr/bin/env python3
"""Before 2026-11-20, zustc's borrow checker gave up after 50 * blocks + 1000 rounds of its fixpoint and then
reported nothing. It now runs to the fixpoint (a bound from the lattice's height: crates/typeck/src/acg/borrowck.rs;
borrowck_props.rs `fixpoint_does_not_give_up`).

Writes a program where K references are shifted along a chain in a loop (`r{i} = r{i-1}`, in reverse order, so each
round of the fixpoint moves the loan of `x` one step), then assigns to `x` while `r{K}` is still used: rustc
rejects it with E0506 for every K. With zustc at 743c2a5 (2026-11-19) the error was found up to K = 259 and missed
from K = 260 on. Usage: borrowck_rounds.py [K] [path to zustc]
"""
import subprocess, sys, tempfile, os

K = int(sys.argv[1]) if len(sys.argv) > 1 else 260
zustc = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "../../target/release/zustc")
lines = ["fn main() {", "    let mut x = 0;", "    let z = 5;", "    let r0 = &x;"]
lines += [f"    let mut r{i} = &z;" for i in range(1, K + 1)]
lines += ["    let mut n = 0;", "    while n < 3 {"]
lines += [f"        r{i} = r{i - 1};" for i in range(K, 0, -1)]
lines += ["        n += 1;", "    }", "    x = 1;", f'    println!("{{}}", r{K});', "}"]
with tempfile.TemporaryDirectory() as d:
    src = os.path.join(d, "rounds.rs")
    open(src, "w").write("\n".join(lines) + "\n")
    r = subprocess.run([zustc, "check", src], capture_output=True, text=True)
    out = r.stdout + r.stderr
    found = "because it is borrowed" in out
    print(f"K = {K}: {'E0506 found' if found else 'missed (accepted)'}")
    sys.exit(0 if found else 1)
