# Placing fields of stack structs in registers (minclif)

Date: 2026-11-16

- The addresses of slots (temporary areas for structs, tuples, arrays) are tracked during analysis: a `stack_addr` value, and that value plus a constant, are
  "(slot, offset)". If that address is used only by load / store / constant addition (passing it to a call, storing it, or
  using it in other computations counts as escaping), reads and writes do not overlap, and the same location always has the same type, then each (slot, offset)
  field becomes one variable (scalar replacement). Allocation treats them like values and variables, and fields that could not be placed in a register
  use their location in the slot. The address value is not created
- acg's struct copy changed from 8 bytes at a time to field by field (with each field's type) (so that read and write widths line up with the fields)
- Slots zeroed at entry (drop flags) are not turned into variables. `MINCLIF_NO_SROA=1` turns this off
- A loop over a struct passed by value and returned by value (the same as tests/acg/struct_fields_regs.rs, 20 million times): 176 ms → 147 ms (rustc -O 87 ms).
  bench/rt barely changes (the structs in nbody and others are inside a Vec: on the heap)
- The self-hosted compiler compiling itself: 9.7 seconds

Upgraded Rust to 1.100.0 and wild to 0.11.0 (`rustup update stable`, `cargo install --locked wild-linker`).
