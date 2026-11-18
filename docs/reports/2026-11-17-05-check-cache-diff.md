# Incremental cache keys and read-back for check (2026-11-17)

Date: 2026-11-17

Every check built cache keys for all functions (9115 in compiler_main.rs) and read them back from the cache (re-requesting the instances
that had been requested). An LSP session now keeps a record of the previous check (`ChkMemo`) and uses it under the following conditions.

- Declaration fingerprint (`Analysis::iface_fp`): for fn items, the hash of the encoded item summary (signature, closures); for other items,
  their text hashes, in order. Editing only a function body does not change it. Everything a key refers to (referenced signatures, structs,
  consts, the name table) is determined by it.
- If the fingerprint is the same as last time, functions whose body item hash is the same are already checked (no key or read-back needed).
  Requested instances are followed by name, and only those whose body (generic function) item changed are re-requested and checked.
  Anything that failed checking is re-checked every time.
- If the fingerprint changed, checking uses keys and the cache as before, and the record is rebuilt. Only sessions that use the cache
  (the LSP) build the record (a one-shot `zustc check` does not). `ZUSTC_NO_CHECK_MEMO=1` disables it.
- tests/check.rs: keeps adding and removing statements that do or do not cause type errors in the function bodies of examples, and each
  time compares the diagnostics with a check in a fresh session.

| Editing 1 function in compiler_main.rs | Before | After |
|---|---:|---:|
| check (keys and read-back, checking) | about 100 ms | 37 ms (13 functions are re-checked: the edited function and the functions with errors) |
| LSP didChange (over stdio) | 238 ms | 194 ms |

Remaining (didChange 194 ms): preprocessing 30 ms, references 29 ms, Program 17 ms, item summary 9 ms, prelude selection 7 ms, split 6 ms,
other (diagnostics JSON, cfg evaluation, etc.) about 60 ms.
