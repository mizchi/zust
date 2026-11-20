# Benchmarks in the shapes of application code

Date: 2026-11-20

The programs the KPIs measured had little variety:

- `bench/gen.py`: 5000 functions of integer arithmetic.
- `bench/gen_traits.py`: generic traits and iterator chains.
- cratebench: seven small crates (fnv, itoa, hex and others: hash functions and number formatting).
- `bench/rt`: seven small runtime programs.

None of them has the code most applications are made of:

- enums with data and the matches over them;
- string processing;
- error handling through `?`;
- trait objects;
- structs that borrow;
- `macro_rules!`;
- module trees.

A change that slows one of those down did not show in the KPIs.

## The patterns

`bench/gen_patterns.py PATTERN N [variant]` writes a program in one of nine shapes. Each is valid Rust (edition
2021), and main prints one checksum per group, so the output can be compared with rustc's. N is the number of
groups: the pattern's code is repeated under its own names, so the program grows linearly. `variant` changes one
constant in the first group, which is the edit of the "edit one function" KPI.

| pattern | code of the kind it stands for | what it exercises |
|---|---|---|
| `interp` | compilers, interpreters, query engines | a lexer over chars; a recursive-descent parser into a Box'ed enum AST; an evaluator with exhaustive matches |
| `records` | CLI tools, log and CSV processing | lines split and parsed into structs (Option, `?`); a HashMap with `entry().or_default()`; `sort_by`; `format!` |
| `errors` | services, request handling | an error enum with Display and From; validation through Result and `?`; Option combinators |
| `state` | protocols, games, workflow engines | enums carrying data; `match (state, event)` with guards; `..` patterns |
| `plugins` | GUI and application frameworks, middleware | `Vec<Box<dyn Trait>>`; default methods; a context shared through `Rc<RefCell<..>>`; `?` on Option |
| `borrows` | parsers, protocol decoders | a struct holding `&'a str`; methods returning slices of the input; `&mut` iteration |
| `macros` | codegen-heavy crates (bitflags-style) | `macro_rules!` generating structs, impls and lookup tables, with repetition and recursion |
| `modules` | the layout of large applications | nested modules; `pub(crate)` / `pub(super)`; `pub use`; `use crate::..`; statics; impls split across modules |
| `generics` | library code | generic containers at several types; traits implemented for tuples, Vec and Option; `IntoIterator` bounds; an iterator adaptor written by hand |

`mix` puts every pattern in one program, N groups each: one program of the shape of an application crate.

## How to measure

- `python3 bench/patterns.py [--n 4] [--instructions]` measures each pattern alone, and `mix`. For each one it
  builds the program with zustc and with rustc and compares their output. It then gives the median times of a cold
  build, `check` and a build after editing one function. With `--instructions`, `check` and the edit build are also
  counted by cachegrind. A pattern that does not build, or prints something else, makes the exit status 1.
- `bench/kpi_track.py` (and so the `KPI` workflow on every pull request) has two new KPIs: `patterns check` and
  `patterns edit`, for `mix 4` (2600 lines).
- `tests/bench_patterns.rs` builds every pattern at N = 2, and `mix` with an edit. It checks that their output is
  rustc's, so a pattern zustc stops building fails `cargo test`.

The patterns at N = 4 (2026-11-20, this machine):

| pattern | lines | same as rustc | cold build ms | check ms | edit 1 fn ms | check M instr | edit M instr |
|---|---:|---:|---:|---:|---:|---:|---:|
| interp | 560 | yes | 54 | 40 | 42 | 133.2 | 53.0 |
| records | 212 | yes | 65 | 38 | 39 | 156.2 | 57.7 |
| errors | 292 | yes | 54 | 28 | 35 | 117.1 | 46.4 |
| state | 260 | yes | 41 | 15 | 28 | 64.9 | 26.0 |
| plugins | 320 | yes | 66 | 47 | 34 | 134.9 | 51.2 |
| borrows | 252 | yes | 47 | 28 | 34 | 102.7 | 43.8 |
| macros | 208 | yes | 39 | 22 | 35 | 86.0 | 41.6 |
| modules | 256 | yes | 46 | 23 | 34 | 84.0 | 36.2 |
| generics | 268 | yes | 60 | 39 | 43 | 134.0 | 56.8 |
| mix | 2596 | yes | 199 | 144 | 77 | 669.2 | 240.0 |

Per line, `records` costs the most to check: 0.74 M instructions a line, against 0.24 for `interp`. Its code goes
through format!, the HashMap entry API and closures over iterators, all of which expand into prelude code.

## What writing them found in zustc

The patterns were first written as Rust that rustc builds, without regard to what zustc supports. Six problems
showed up. Two are fixed here, each with a test in `tests/probe` whose expected output is rustc's.

1. **Fixed:** a method on a static reached through a module path (`config::NAMES.len()`, `config::NAMES[i].len()`,
   `super::config::NAMES[i]`) was "no method `len` on type `str`". `peek_syn`, which gives an expression's type
   before it is compiled, did not read such a path, so the method was looked up on the wrong type. It now reads the
   static the path names, as `expr` and `place` already did (`crates/typeck/src/acg/mod.rs`).
   Test: `tests/probe/static_path_method.rs`.
2. **Fixed:** a type parameter given only by `I: IntoIterator<Item = A>` could not be inferred when the argument was
   a Vec or a HashMap ("cannot infer type parameter `A`"). The prelude's collections have an `into_iter` method, not
   an IntoIterator impl, so there was no `Item` to read. The Item is now that of the iterator `into_iter` returns
   (`solve_assoc_bounds`, `crates/typeck/src/acg/p1_expr.rs`). An array argument is not covered.
   Test: `tests/probe/into_iterator_item.rs`.
3. **Open, wrong code:** two modules that each define a trait of the same name, and implement it for the same type,
   share one impl. zustc does not tell the traits apart. With
   `mod a { trait M { .. } impl M for u32 { .. 1 } }` and `mod b { trait M { .. } impl M for u32 { .. 2 } }`,
   both modules call one of the impls: zustc prints `5 5` where rustc prints `5 10`. This is the most serious of the
   six, since the program builds.
4. **Open:** a closure is named after the type and the method it is in (`Vec::m::{closure#0}`), not after its trait or
   module. Two traits whose impls for Vec both have a method `m` with a closure in it fail with "duplicate
   definition of `Vec::m::{closure#0}::call`". Renaming the second closure turns this into the wrong code of 3, so it
   should be fixed together with 3.
5. **Open:** `use super::model;` followed by `model::make()` is "cannot find value (unsupported path)" when another
   module named `model` exists elsewhere in the crate. `use super::model::make;` and `super::model::make()` work.
6. **Open:** a trait's method as a value, as in `.map(Tr::m)` or `.map(ToString::to_string)`, is "cannot find value
   (unsupported path)".

Also seen while reducing these: `fn f<I: IntoIterator>(a: I) -> Option<I::Item>` makes zustc panic
(`crates/syntax/src/types_enc.rs:11`), and a Range is not accepted where `IntoIterator` is bound.

The generator works around 3–6, so the patterns can be measured now. Each workaround is commented in
`bench/gen_patterns.py`:

- `generics` names its trait and method per group;
- `generics` passes a closure, not `Tr::m`;
- `modules` imports the item, not the module.

Once one of these is fixed, the workaround can be removed and the pattern covers that shape too.

A limit of the output check: the groups of one pattern differ only in a few constants. A program in which zustc
mixed up two groups' items (as in 3) can still print the right checksums. The probes above were made by changing one
group by hand.

## What was checked

- Every pattern and `mix`, at N = 1, 2, 3 and 4, with and without an edit: zustc's output is rustc's.
- `tests/bench_patterns.rs`, and the two probes in `tests/driver2.rs`.
- `cargo test --release --workspace`, `bench/rust_ui.py --check`.
- KPIs: the `KPI` workflow's comment on the pull request has the change against the base. `patterns check` and
  `patterns edit` are new, so the base has no value for them.
