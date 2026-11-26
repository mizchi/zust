# zust-ir

The instruction stream between the type checker and the backends: types, values, blocks, variables, stack slots,
function references, relocations, and the recorded instructions (`Op`).

zust-typeck records into a `Recorder` as it checks a fn. A backend (minclif, Cranelift, wasm) reads the recording
back and generates code; `check` drops it. No dependencies, so zustc can compile this crate itself.
