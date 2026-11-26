# formal/

Formal models and bounded proofs of parts of zustc, with [idealis](https://github.com/mizchi/idealis) (TLA+
model checking in Rust: rtla → rtla-compiler → rtlc) and [Kani](https://github.com/model-checking/kani) (bounded
model checking of Rust code with CBMC). What they found and the prior work they follow:
[docs/reports/2026-11-19-01-formal-models.md](../docs/reports/2026-11-19-01-formal-models.md) ([日本語](../docs/ja/reports/2026-11-19-01-formal-models.md)).

Nothing here is part of the zust workspace or of the zustc build.

| path | what | tool |
|---|---|---|
| `tla/CacheSave.tla` | two zustc processes saving the build cache (`BuildCache::save`: write a tmp file of the process's own, rename) and a reader (`BuildCache::open`) | idealis |
| `tla/ForkPool.tla` | the fork/pipe worker pool of the driver: fork failure, a child dying mid-record, full pipes, inherited write ends | idealis |
| `tla/ArcDrop.tla` | the prelude's `Arc` refcount if zustc had threads, against `fetch_sub` | idealis |
| `kani-prelude/` | the prelude's `Vec<T>`, copied from `crates/syntax/src/prelude_lib.rs.in` by `build.rs`, over a model of `realloc` / `free` | Kani |
| `../crates/typeck/src/acg/borrowck_props.rs` | the lattice of the borrow checker's dataflow (`add_sorted`, `State::join`, `prefix` / `overlap`), tried on every input of a small universe (first written for Kani: CBMC runs out of memory on `Vec::insert` at a symbolic index) | `cargo test -p zust-typeck props` |
| `repro/` | programs that show the defects found (zustc against rustc) | |
| `tla-check/` | the runner: parses a `.tla` / `.cfg` with idealis (pinned by git rev) and model-checks it | |
| `tla/HashMapModel.tla` | what a HashMap does (insert / remove / get / len over 5 keys), as the model `connect/` tests the prelude's HashMap against | idealis |
| `connect/` | the implementation against the models, in the way of Quint Connect: with idealis-connect: a spec's state graph becomes an idealis `FiniteModel`, action sequences are generated from it (a failure is shrunk), and each runs on the model and on a driver | idealis |
| `lean/` | the trait solver (`crates/sema/src/traits.rs`): `Holds` (inductive) and the algorithm `solve`, with proofs that its `yes` and `no` are sound, that it is complete with enough fuel, that leaving out what zustc cannot read keeps `no` sound, and that overlap of linear impl heads is decided exactly ([lean/README.md](lean/README.md)) | Lean 4 |

```bash
formal/run.sh tla
```

```bash
formal/run.sh connect
```

```bash
cargo install --locked kani-verifier && cargo kani setup
```

```bash
formal/run.sh kani
```

```bash
formal/run.sh lean
```

`run.sh` checks every result against what is expected. Configurations that model a design zustc does not use (or
no longer uses) must find its bug: `CacheSaveShared.cfg` (one tmp name for every process, as before 2026-11-20),
`ForkPoolCloseLate.cfg` (closing the pipe write ends late), `ArcDropPrelude.cfg` (the prelude's `Arc` if there were
threads). The others, and every Kani harness, must pass. The TLA+ checks take milliseconds; the Kani harnesses take
about 8 minutes and 5–10 GiB.
`formal/run.sh` with no argument also runs `cargo test -p zust-typeck props`.

## connect: does the implementation follow the model?

`connect/` (`zust-connect`) checks an implementation against a TLA+ spec with idealis-connect (its `tla` feature):

1. `tla::finite_model` model-checks the spec and makes its reachable state graph an idealis `FiniteModel`. The spec
   names each step in a variable `act` (the action and its arguments) and what the step observes in `obs`; the states
   are keyed by the other variables. `CONSTRAINT` and `ACTION_CONSTRAINT` prune the graph as they prune the search.
2. `tla::check` generates action sequences from that graph (`Generator::from_finite_model`: random walks from a seed,
   a failure shrunk to a shortest sequence) and runs each through idealis-core's conformance session. The model gives
   the observations it allows after each action; a `tla::Driver` runs the action on the implementation and returns
   what it observed. The expected observations come from the spec, never from the implementation. A sequence that a
   nondeterministic observation leaves the model on is invalid, not a failure.

`connect/tests/hashmap.rs` builds `connect/drivers/hashmap.rs` with `target/release/zustc` and drives it over
stdin / stdout, so what is checked is the prelude's HashMap as zustc compiles it: 200 sequences of up to 100 steps
against `tla/HashMapModel.tla`. A second test checks that a map whose `remove` does nothing is caught: the failure
shrinks to `insert 5 2`, `remove 5`, `get 5` (which sees 2 where the model allows only 0).

A new target is a spec with `act` / `obs` and a `Driver`. The generator takes up to 1024 states and 10,000
transitions, so the state the implementation is compared on has to stay that small.
