# Cache and parallelization in driver2

Date: 2026-11-16

- `src/cache2.rs`: the key is a 128-bit hash; the value is either an item summary (the serialization of Summary in `types_enc.rs`) or a function's machine code +
  symbolic relocations (acg::Blob; callee instances are written as "original function name + type arguments"). Only the entries used this time are saved
- A function's key is "hash of the body + signatures of itself and its callees + layouts of structs that appear by name + consts" (the same idea as driver.rs)
- Parallelization uses fork: child process k compiles the functions with i % jobs == k and returns the Blobs to the parent over a pipe. The parent reads them back in order and
  re-requests the instances (the .o is byte-identical with j=1 and j=8). zustc's std has no threads, so it calls libc's fork / pipe / waitpid directly
- A 400-function program: cold 104 ms (j=1) / 90 ms (j=8), no changes 7 ms (no linking), changing 1 function recompiles only that 1 function
