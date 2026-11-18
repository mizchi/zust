# matmul and sieve with `-O`, generics and monomorphization

Date: 2026-11-15

| | Before | After | rustc -O |
|---|---:|---:|---:|
| matmul `-O` | 77 ms | **53 ms** | 31 ms |
| sieve `-O` | 252 ms | **144 ms** | 92 ms |
| nbody `-O` | 92 ms | **69 ms** | 42 ms |

- Hoisting out of loops (`-O`): the start and length of indexed receivers that do not change in the body (and condition) of a for / while are read before the loop.
  - Targets are local Vecs, `&Vec` / `&mut Vec` arguments (`c` / `b`), and rows indexed by variables that do not change in the loop (`c[i]` / `b[k]`).
    The receiver must appear in the body only in the form `x[..]..` (a read, or an assignment to a scalar element), with no method receiver, `&mut`, re-binding, macro or closure.
  - The row is read before the loop; if out of range, the read location becomes a dummy area (`select`). The check `i < c.len()` is done inside the body, using the value compared beforehand (the order of panics does not change).
  - The inner loop of matmul went from 30 instructions → 12 instructions (the re-reads of `c` and `c[i]` disappeared).
- `vec![x; n]` no longer calls push and writes directly into the allocated area (memset for 1-byte elements). The 20 million pushes in sieve's initialization disappeared.
- `for (i, x) in v.iter().enumerate()` binds i and x without building a tuple (the stores to the stack and the reloads disappeared; sieve's instruction count went from 615M → 535M).
- Instances that produce the same machine code are placed only once (ICF: `Vec<u32>::push` and `Vec<i32>::push`, etc.). Instances whose machine code, relocations (callees rewritten to the representative), data, vtables, panic locations and calling convention are the same are folded, repeating until nothing changes.
  The cache looks up machine code in the `.o` by (length, hash), so folded instances also hit with the representative's machine code. Across all examples, binaries −3% (binary_heap −13%), `.o` −3%.
- `__exr_drop_in_place::<T>` / `drop(x)` are not called for types that need no drop (and no instances are created; collections went from 353 → 341 instances).
