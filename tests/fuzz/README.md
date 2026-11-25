# tests/fuzz

Inputs that zust-fuzz (crates/fuzz) found to panic, hang, crash or make the two macro expanders disagree, minimized
and kept after the fix. `crates/fuzz/tests/regressions.rs` runs each one through every target and fails on any
panic or mismatch. They also seed the fuzzer's corpus.

A file's first line may give its edition (`// edition: 2015`); the default is 2021. Name a file by what it exercises.
