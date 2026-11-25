# crates/

zustc is split into layers. Each crate depends only on the ones below it, so the editor tooling (zust-analyzer) can
use the front end and the type checker without the code generator.

```
                         zustc (src/: CLI)        zurgo (Cargo-compatible driver)
                                   \                 /
  zust-analyzer (LSP) ──────────── zust-core (session: Session, cache, build)
        │                            │            │
     zust-ide                   zust-typeck   zust-codegen ── zust-clif (Cranelift)
        │                            │            │           zust-wasm  (wasm32)
        │                        zust-sema     minclif (x86-64 ELF backend)
        │                            │            │
        └──────────────────────  zust-syntax   zust-ir (recorded instructions)
                                     │
                                 zust-base
```

| crate | directory | responsibility |
|---|---|---|
| zust-base | [base](base/README.md) | small shared parts with no dependencies: FxHash maps, cache keys, phase timers |
| zust-syntax | [syntax](syntax/README.md) | source to items: cfg, `macro_rules!`, module flattening, the parser, item summaries, the built-in prelude |
| zust-sema | [sema](sema/README.md) | the program's tables (fns, types, traits, impls), the trait solver, type inference of fn bodies, const evaluation |
| zust-typeck | [typeck](typeck/README.md) | the type check of each fn body (acg), which records instructions; borrow and region checks; the per-fn cache format |
| zust-ir | [ir](ir/README.md) | the recorded instruction stream that typeck writes and the backends read |
| minclif | [minclif](minclif/README.md) | the minimal x86-64 backend with a Cranelift-like API; self-hostable |
| zust-codegen | [codegen](codegen/README.md) | replays the recorded instructions into a backend; invokes the linker |
| zust-clif | [clif](clif/README.md) | the Cranelift backend (same recorded stream, another code generator) |
| zust-wasm | [wasm](wasm/README.md) | the wasm32-unknown-unknown backend |
| zust-core | [core](core/README.md) | the driver: `Session` (check, analysis, caches) and the build (parallel compile, objects, link, workspaces) |
| zust-ide | [ide](ide/README.md) | editor analyses from tokens alone: symbols, definitions, references, hover, completion |
| zust-analyzer | [analyzer](analyzer/README.md) | the LSP server (stdio) |
| zust-fuzz | [fuzz](fuzz/README.md) | mutation-based fuzzing of the front end and the checker: panics, hangs, crashes, the two macro expanders against each other |
| zurgo | [zurgo](zurgo/README.md) | a Cargo-compatible build driver for zustc: manifests, lockfiles, features, build scripts, `zurgo test` |
| zustscript | [zustscript](zustscript/README.md) | an experimental language on top of zust; outside the workspace, with its own CI |

Rules that hold across the crates:

- No dependency points up. A check that needs the driver's view lives in zust-core, not in a lower crate.
- zust-base, zust-ir and minclif use no crates.io dependency. They, and the modules `tests/selfhost.sh` lists (the lexer,
  parser, `macro_rules!` expander, prelude selection, program tables, cache encoding), must stay within the Rust that
  zustc compiles, because the self-host test compiles them with zustc.
- Lower crates re-export what they build on (`zust_core::lexer`, `crate::fxhash` inside zust-syntax), because the
  self-host build puts every module of the compiler into one crate and the paths must resolve the same way there.
- Each crate is measured by the KPIs in `bench/kpi_track.py` (instruction counts of `check -j 1`, builds, the LSP
  session, the build cache size). A change to a hot path is measured before and after (see CONTRIBUTING.md).
