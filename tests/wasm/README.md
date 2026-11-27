# wasm32-unknown-unknown in a sandbox

`zustc --target wasm32-unknown-unknown x.rs -o x.wasm` writes one wasm module (issue #7). The backend is
`crates/wasm`: it lowers the same recorded instructions the Cranelift and minclif backends read, so caching,
incremental rebuilds and the fork-parallel compile work as for native builds (a cold build of examples/collections.rs
takes 51 ms, an edit of one function 16 ms).

`tests/wasm.rs` (`cargo test --release --test wasm`; needs node and rustc) checks that the module runs in a sandbox and
stays in it:

| test | what it checks |
|---|---|
| `wasm_runs_like_rustc` | examples/*.rs and the runtime cases of tests/security (CVE regressions, bounds, overflow, capacity overflow, UTF-8) print what rustc's builds print, with the same exit codes |
| `wasm_workspace_runs_like_native` | `tests/workspace` built with `zustc build --target wasm32-unknown-unknown` (one module from three crates) prints what its x86-64 build prints, with the same exit codes |
| `wasm_module_shape` | a module imports only functions of `env` that the host gives, exports only `memory` and `main`, and has no start function |
| `wasm_host_denies` | `deny/*.rs`: spawning a process (the module imports `fork` / `execvp` and is not run at all), reading and writing host files (they fail; no file appears), the host's environment (empty) |
| `wasm_contains_faults` | `contain/*.rs`: unsafe code that writes past 4 GiB (would wrap onto the stack), reads the null page, writes past the end of memory, copies or fills such addresses in bulk, calls an integer as a function, calls a function pointer with bits past 32 set, calls through a pointer of another signature, overflows the stack, or allocates more than memory: each run ends in a trap or an abort, before its next line |

## SIMD (`core::arch::wasm32`, issue #30)

zustc's IR has vector types of a fixed width, `Type::Vec(VBits)` (128, 256 or 512 bits), and SIMD instructions
written per lane, not per width: `Op::Simd(Shape, VOp, ..)`, where the `Shape` is the lane type and the width
(`f32x4.add` is `Add` on f32 lanes of 128 bits; AVX2's `vaddps ymm` would be the same `Add` at 256). The set is what
wasm SIMD and SSE4.1 / AVX2 / NEON have in common, with the relaxed instructions apart. Only the byte shuffles
(`Shuffle`, `Swizzle`) and `Vconst` are 128-bit only. The wasm backend emits 128-bit vectors only: each instruction as
its `0xFD` opcode (the relaxed ones from `0xFD 0x100`), and a wider vector is an error, not split into several.
Node 22 and 26 run both sets without flags.

The design leaves room for wasm's flexible-vectors proposal (phase 3): its types (`vec.i32` ..) have a length the
engine chooses, a runtime constant read by `vec.i32.length`, and its operations are lane-wise ones of the same kinds
(arithmetic, compares to masks, `narrow`, `widen_low` / `widen_high`, splat, lane access by immediate within the
low 128 bits), without a byte shuffle. That would be one more width whose size is the target's at run time, like
Cranelift's dynamic vector types: vectors that can be locals and parameters but not fields of a laid-out struct, plus
an instruction for the lane count and the lane shifts and half concatenations (`crates/ir`, the SIMD section).

- `v128` is a primitive type (`Ty::Vec(128)`: 16 bytes, aligned to 16), held in one wasm local, passed and returned as a `v128`
  parameter or result. It can be a struct field, an array or `Vec` element, an `Option` payload, a closure capture or a
  generic argument.
- The intrinsics of `core::arch::wasm32` (rustc's names and types: `f32x4_add`, `u8x16_shr`, `i32x4_extract_lane::<N>`,
  `i8x16_shuffle::<..>`, `v128_load`, `f32x4_relaxed_madd`, ..: the whole `simd128` and `relaxed-simd` sets) are
  lowered in place to one instruction each, or a few (a constructor is a splat and lane replacements). They are not
  prelude functions: a program that names none of them compiles as before. One that defines a function of the same
  name calls its own. An intrinsic cannot be used as a function value (`apply(f32x4_add, ..)`: write a closure).
- `cfg(target_feature = "simd128")` and `"relaxed-simd"` are true for wasm32.
- Loads and stores of `v128` check their address as scalar ones do (`contain/simd_*.rs`).
- Native builds: the Cranelift backend emits the same instructions (128- and 256-bit vectors, `core::arch::x86_64`
  too: tests/simd/README.md), and `core::arch::wasm32`'s intrinsics give the wasm results natively. minclif rejects a
  function with a vector.

`tests/wasm.rs`'s `wasm_simd_like_rustc` checks that `simd/*.rs` (every intrinsic, on edge cases: NaN, ±0, saturation,
out-of-range shifts and swizzle indices, and `v128` as a value) print what rustc's wasm32-wasip1 builds print under
Node's WASI (`wasi.mjs`; needs `rustup target add wasm32-wasip1`). rustc's build is unoptimized there: with -O, LLVM
folds `f32x4_pmin` / `f32x4_pmax` of constants with the operands swapped, which differs from its definition and
from the wasm spec on NaN and ±0 lanes. Relaxed results are compared on inputs where every allowed result agrees, and
by whether a result is one of the allowed ones.

The kernels of experiments/zustscript written against `core::arch::wasm32` (`simd/kernels.rs` has them), timed
in place of zsc's hand-written v128 module with the same JS glue (`tests/ml_bench.mjs`, Node 22.23, 1e6 elements;
zustc without -O):

| kernel | hand-written v128 | zustc, `core::arch::wasm32` | zustc, scalar (before) |
|---|---|---|---|
| dense 512×512 | 0.11 ms | 0.20 ms | 3.69 ms |
| relu | 2.7 ms | 4.0 ms | 42.0 ms |
| saxpy (relaxed madd) | 2.1 ms | 3.7–4.1 ms | 29.0 ms |
| sum of squares | 0.55 ms | 0.91 ms | 14.6 ms |

The instructions are the same as the hand-written module's; the 1.5–1.8x left is the backend's: the `loop` +
`br_table` dispatcher between blocks and the address checks (with `ZUSTC_WASM_NO_ADDR_CHECK=1` and -O, the sum of
squares takes 0.70 ms), and zustc's `vec![0.0; n]` for the outputs.

## The sandbox

`run.mjs` is the host. The module can do only what the host's imports let it:

- `write` to fd 1 and 2, `putchar` (buffered as glibc buffers it);
- `open` / `read` / `close` of one virtual file, `/proc/self/cmdline` (std::env::args reads it);
- `exit` / `_exit` / `abort`; `sigaction` and `fflush` as no-ops; `getenv` (always null);
- pure functions: `Math`, `strtod` / `strtof`, `memmem`.

A module that imports anything else is refused before it runs (exit 126, naming the imports). The allocator and
`memcpy` / `memmove` / `memset` / `strlen` / `strerror` / `__errno_location` are inside the module.

Inside the module, `crates/wasm` keeps memory errors from turning into silent corruption:

- linear memory is laid out as a 4 KiB null page, the shadow stack (1 MiB, growing down towards the null page), the
  data, then the heap; memory never grows past 2 GiB;
- every load and store checks its address with one compare: an address that does not fit in 32 bits (narrowed to a
  wasm address it would wrap onto other memory: pointers stay 64-bit, as on x86-64) or that falls in the null page
  traps. The bulk copies and fills (`memcpy` / `memmove` / `memset`) check their operands the same way, and a
  function pointer must fit in 32 bits before it is narrowed to a table index;
- a frame is aligned to its strictest slot, and a frame larger than the stack is a compile error;
- every function entry checks the stack pointer: a stack overflow traps instead of running into the data;
- function pointers are table indices and `call_indirect` checks the signature, so a forged pointer cannot jump into
  arbitrary code;
- `malloc` returns null for a request larger than 1 GiB or when memory cannot grow (the prelude then aborts).

Exit codes of `run.mjs`: the program's own; 101 for a trap at `unreachable` (a panic, an overflow or bounds check, a
failed address check); 139 for an access past the end of memory; 134 for other traps (the engine's call stack, a
table index or signature mismatch) and for `abort`; 126 for a module that needs imports the host does not give.

## Differences from rustc's wasm32 output, and what is left (also in [TODO.md](../../TODO.md))

- `usize` and pointers are 8 bytes (the layouts are those of x86-64), so `cfg(target_pointer_width = "64")` is true.
  Exported functions take and return i64 for them.
- Traps carry no panic message (the native build prints the location from its trap table).
- Not supported yet: i128 division and float conversions, threads (`spawn` needs pthreads), processes,
  files other than the arguments, and self-hosting zustc as wasm.
- Speed: every block is a case of one `loop` + `br_table` dispatcher and every value a local, so bench/rt runs 3 to 15
  times slower than the native build under Node (the address checks are 5–20% of that). A relooper / stackifier and
  local reuse are the next steps.
