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
| `wasm_module_shape` | a module imports only functions of `env` that the host gives, exports only `memory` and `main`, and has no start function |
| `wasm_host_denies` | `deny/*.rs`: spawning a process (the module imports `fork` / `execvp` and is not run at all), reading and writing host files (they fail; no file appears), the host's environment (empty) |
| `wasm_contains_faults` | `contain/*.rs`: unsafe code that writes past 4 GiB (would wrap onto the stack), reads the null page, writes past the end of memory, calls an integer as a function, calls through a pointer of another signature, overflows the stack, or allocates more than memory: each run ends in a trap or an abort, before its next line |

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
  traps;
- every function entry checks the stack pointer: a stack overflow traps instead of running into the data;
- function pointers are table indices and `call_indirect` checks the signature, so a forged pointer cannot jump into
  arbitrary code;
- `malloc` returns null for a request larger than 1 GiB or when memory cannot grow (the prelude then aborts).

Exit codes of `run.mjs`: the program's own; 101 for a trap at `unreachable` (a panic, an overflow or bounds check, a
failed address check); 139 for an access past the end of memory; 134 for other traps (the engine's call stack, a
table index or signature mismatch) and for `abort`; 126 for a module that needs imports the host does not give.

## Differences from rustc's wasm32 output, and what is left

- `usize` and pointers are 8 bytes (the layouts are those of x86-64), so `cfg(target_pointer_width = "64")` is true.
  Exported functions take and return i64 for them.
- Traps carry no panic message (the native build prints the location from its trap table).
- Not supported yet: i128 division and float conversions, threads (green threads need `swapcontext`), processes,
  files other than the arguments, workspace builds (`zustc build`), and self-hosting zustc as wasm.
- Speed: every block is a case of one `loop` + `br_table` dispatcher and every value a local, so bench/rt runs 3 to 15
  times slower than the native build under Node (the address checks are 5–20% of that). A relooper / stackifier and
  local reuse are the next steps.
