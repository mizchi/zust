# zust-base

Small parts that every layer of zustc uses. No dependencies, so zustc can compile this crate itself.

| module | what |
|---|---|
| `fxhash` | `FxHasher`, `FxMap`, `FxSet`: the fast non-cryptographic hash used for name tables |
| `key` | cache keys (`Key`, `KeyHasher`, SipHash) for the per-item and per-fn caches |
| `prof` | per-phase timers (`ZUSTC_PROF=1`), without std::time |

Used by: zust-syntax and everything above it, which re-export these modules so `crate::fxhash` resolves the same
way in the self-host build.
