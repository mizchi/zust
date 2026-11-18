# Register allocation in minclif

Date: 2026-11-16

minclif turned instructions into machine code immediately and placed every value in a stack slot (each instruction loads into rax / rcx and writes the result back).
Allocation needs to see the whole function, so it now assembles twice (`src/minclif.rs`):

1. **Recording and analysis**: `FunctionBuilder` records each called instruction as an `Op` (104 kinds, generated mechanically from Emitter's public methods) while
   assembling a first pass with the existing assembler (`Emitter`) (it decides value numbers and types, and returns the values and blocks that acg queries).
   The first pass records value and variable reads and writes (instruction numbers), segments (ranges where a block was written contiguously) and jump targets, and instructions that call functions
2. **Allocation** (`plan`): for a value written and read inside one segment, the live interval is that range; for values that cross segments (variables, block parameters, values live in loops),
   the range is extended to the end of the live segments using backward dataflow (whether it is live at segment entry and exit). Linear scan then
   assigns them to rbx, r12 to r15 (callee-saved: for intervals that cross calls) and rsi, rdi, r8 to r10 (for intervals with no calls).
   If registers run out, those with a small "number of reads and writes / length" go to slots. Floating point, i128, and values that use a slot directly (used
   as an address, or read and written with different widths) stay in slots. A call immediately followed by a trap (panic) does not return, so it does not count as crossing
3. **Final pass**: the Emitter, now holding the allocation, assembles the recorded instructions again. The existing instruction assembly is reused as-is,
   and value loads and stores (load_rax / store_rax, etc.) consult the allocation. Common instructions are written in register form: 64 / 32 bit
   add / sub / and / or / xor / imul and cmp become `mov d, a; op d, b`, constants are used directly, and load / store use `[base + off]`.
   Callee-saved registers that are used are saved in the prologue and restored before ret. When call arguments are in argument registers, they
   go through the stack (writing them in order would clobber them)

Also: if an `icmp` value is used only by `brif`, it becomes `cmp` + `jcc`; if nothing is passed to else, jump directly; a `jmp` to the immediately next block is a nop.
Floating point values are placed in xmm8 to xmm15 (clobbered by calls, so only for intervals with no calls), and `fadd` and similar also read the second operand from a register.
Copies between variables and values are direct. When calling our own functions (`_x..`), `al` for variadic arguments is not written.
`MINCLIF_NO_REGALLOC=1` disables allocation (for comparison).

Results:
- Run times from `python3 bench/runtime.py` (driver2 -O. All outputs match):

  | | no allocation | integers only | + floating point, direct copies | Cranelift path (`zustc -O`) | rustc -O |
  |---|---:|---:|---:|---:|---:|
  | fib | 111 ms | 90 | 96 | 82 | 30 |
  | hashmap | 341 | 210 | 208 | 149 | 103 |
  | matmul | 242 | 190 | 158 | 91 | 45 |
  | nbody | 1150 | 725 | 516 | 83 | 47 |
  | sieve | 346 | 185 | 169 | 141 | 109 |
  | sort | 624 | 344 | 341 | 252 | 210 |
  | strings | 227 | 119 | 120 | 70 | 42 |

  All are faster than rustc's debug build, and the build takes about half the time of the Cranelift path. Execution has not yet caught up with the Cranelift path
  (struct fields and array elements stay in memory, and instruction selection is still simple)
- A simple loop (sum of 1000 elements × 200 thousand times): 1.08 seconds → 0.44 seconds (the same as rustc opt-level=0)
- Time for the self-hosted compiler to compile itself: 18.0 seconds → **10.1 seconds** (front 3.2 → 1.7 seconds, codegen 13.0 → 7.6 seconds).
  The compiler executable went from 23 MB → 12 MB
- Added wild (0.10) as the linker (`cargo install --locked wild-linker --version 0.10.0`. link.rs uses wild > mold > ld.lld in that order).
  Workspace benchmark: cold 355 ms, function body change 37 ms, bin change 60 ms
