# Self-hosting stage 16: the foundation of a backend to replace Cranelift

Date: 2026-11-16

- `src/minclif.rs`: a minimal backend with an API modeled on Cranelift's FunctionBuilder (blocks, block parameters, variables, stack slots, integer instructions, comparisons, select,
  loads and stores, calls (System V, the 7th argument and later on the stack), return). It emits x86-64 machine code directly and writes an ELF relocatable object.
  Code generation is simple: every value lives in a frame slot. `tests/minclif.rs` builds loops, recursion and an 8-argument function, links them with cc and runs them.
  The version compiled with zustc also writes a .o with identical bytes, and linking it works (minclif in tests/selfhost.sh)
- Supported up to: floating point (f32 / f64 arithmetic, sqrt, rounding, comparisons (including NaN handling), sign operations, conversion to and from integers (saturating, like Rust's `as`)),
  i128 (add/sub, bitwise ops, comparisons and shifts as instruction sequences; mul/div via libgcc's `__multi3`, etc.), operations with overflow, clz / ctz / popcnt /
  bswap / rotate / min / max / abs / umulhi, trapz / trapnz, calls through function pointers, addresses of names (via the GOT),
  floating-point arguments and return values (xmm), .data and pointers in it (R_X86_64_64). ext_ops in `tests/minclif.rs` compares 69 results with rustc's computation
