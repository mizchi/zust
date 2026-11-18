# Smaller IR passed to Cranelift (2026-11-17)

Date: 2026-11-17

Totals of the IR passed to Cranelift, and times, when compiling the compiler itself (out/selfhost/compiler_main.rs) with `--no-cache -j 1`.

| | Before | After |
|---|---:|---:|
| Values | 837,051 | 785,840 (−6%) |
| Stack slots | 74,805 | 62,674 (−16%) |
| Instructions | 972,578 | 997,071 (+3%: from rebuilding slot addresses where they are used) |
| Cranelift compile time | 1420–1490 ms | 1270–1320 ms (−11%) |
| Total | 3850–4020 ms | 3670–3820 ms (−5%) |
| .o of the resulting compiler | 5.20 MB | 4.54 MB (−13%) |

The time for the resulting compiler to compile the same file does not change (2.37–2.52 s). The run times of bench/runtime.py are the same or slightly faster.

- A temporary area that a call wrote its return value into (and a temporary area where a struct / enum was built) is remembered as
  "not used by anyone yet". By-value arguments and `let` use it directly without copying (`Cx::fresh` / `take_fresh`. Values held in
  temporaries that are dropped at the end of the statement are excluded).
- If the tail expression of a function or the expression of a `return` is a call or a struct / enum value, its return value is written
  directly into the function's own sret location (`Cx::ret_call` / `dst_hint`. This is not passed to calls inside arguments).
- clif backend: slot addresses (`stack_addr`) are rebuilt where they are used. Their live ranges get shorter, and they do not become
  Variables that cross ranges (block arguments).
- clif backend: a block is sealed as soon as all instructions that jump into it have been written. Variable values can be resolved on
  the spot, so there are fewer placeholder block arguments. A branch on a constant condition becomes a jump.
- Tried and dropped: turning a copy made of two consecutive 8-byte load / store pairs into one 16-byte (xmm) pair. Instructions went
  down 10% but the time did not change. Reading 16 bytes right after writing 8 bytes defeats store-to-load forwarding, and the
  resulting compiler ran 20% slower.
- Remaining: even for small functions, Cranelift takes about 30 µs per function (50 ms for 1600 functions under 10 instructions).
  Most of the time is regalloc2's live range computation (Env::init) and allocation.
- Environment variables for investigation: `ZUSTC_CLIF_STATS=1` (count per instruction kind), `ZUSTC_CLIF_DUMP=1` (IR), `ZUSTC_CLIF_TIME=1`
  (compile time per function). Each change can be turned off with `ZUSTC_NO_FRESH` / `ZUSTC_NO_RET_DST` / `ZUSTC_CLIF_NO_REMAT` /
  `ZUSTC_CLIF_LATE_SEAL` / `ZUSTC_CLIF_NO_FOLD_BR`.
