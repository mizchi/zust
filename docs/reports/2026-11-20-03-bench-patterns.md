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

- `python3 bench/patterns.py [--n 4] [--instructions]` measures each pattern alone, and `mix`. Run it from the
  repository root after `cargo build --release`, since it runs `target/release/zustc`.
  - For each pattern it builds the program with zustc and with rustc, and compares their output. rustc must be on
    PATH. With `--no-rustc` the comparison is skipped, and then a successful run checks nothing.
  - It then gives the median times of a cold build, `check` and a build after editing one function. With
    `--instructions`, `check` and the edit build are also counted by cachegrind.
  - The exit status is 1 when a pattern or its edit does not build, a program exits with an error, or the output
    differs.
  - It only prints the table (and `--json`). `bench/kpi_track.py` is what appends rows to the KPI log.
- `bench/kpi_track.py` (and so the `KPI` workflow on every pull request) has two new KPIs: `patterns check` and
  `patterns edit`, for `mix 4` (2600 lines).
- `tests/bench_patterns.rs` builds every pattern at N = 2, and `mix` with an edit. It checks that their output is
  rustc's, so a pattern zustc stops building fails `cargo test`. That holds where python3 and rustc are on PATH, as
  in CI; without them the test says so and passes. It is part of the default `acg` feature.

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
showed up. Four are fixed here, each with a test in `tests/probe` whose expected output is rustc's.

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
3. **Fixed (it was wrong code):** two modules that each define a trait of the same name, and implement it for the same
   type, shared one impl. With `mod a { trait M { .. } impl M for u32 { .. 1 } }` and
   `mod b { trait M { .. } impl M for u32 { .. 2 } }`, both modules called the first impl: zustc printed `5 5` where
   rustc prints `5 10`. This was the most serious of the six, since the program built.
   - Flattening the modules renames the traits (`a__M`, `b__M`), but a method call names no trait. Which impl it
     means depends on the trait in scope at the call, and flattening loses the modules.
   - `lexer::trait_scopes` now reads, from the source before flattening, the traits in scope in each module. That is
     the traits defined there, and the ones brought in by `use`, `use .. as` and globs. It does this only when two
     scopes define traits of one name; otherwise a quick look at the text ends it, without tokenizing.
   - `pick_overload`, which chooses among the impls of one method name, takes the one whose trait is in scope at the
     calling function's line.
4. **Fixed:** a closure is named after the type and the method it is in (`Vec::m::{closure#0}`), not after its trait.
   Two traits whose impls for Vec both have a method `m` with a closure in it failed with "duplicate definition of
   `Vec::m::{closure#0}::call`".
   - The second such closure now gets its item's number in its name, in the types of its `call`, and in its symbol.
     The symbol is `<the fn's symbol>.c<k>`, which was the same for the two, so one closure's code was used for both.
   - Only programs that were an error before are renamed.
   - Test for 3 and 4: `tests/probe/same_trait_two_modules.rs`. It covers traits defined in the module, `use super::M`,
     `use crate::b::M as BM`, `use crate::a::*`, a root `use`, and closures in both impls. It also passes with `-O`
     and with minclif.
5. **Open:** `use super::model;` followed by `model::make()` is "cannot find value (unsupported path)" when another
   module named `model` exists elsewhere in the crate. `use super::model::make;` and `super::model::make()` work.
6. **Open:** a trait's method as a value, as in `.map(Tr::m)` or `.map(ToString::to_string)`, is "cannot find value
   (unsupported path)".

Also seen while reducing these: `fn f<I: IntoIterator>(a: I) -> Option<I::Item>` makes zustc panic
(`crates/syntax/src/types_enc.rs:11`), and a Range is not accepted where `IntoIterator` is bound.

The generator works around 5 and 6, so the patterns can be measured now. Each workaround is commented in
`bench/gen_patterns.py`:

- `generics` passes a closure, not `Tr::m`;
- `modules` imports the item, not the module.

Once one of these is fixed, the workaround can be removed and the pattern covers that shape too. Since 3 and 4 are
fixed, `generics` again has one trait `Measure` per group for the same types (it had named them per group).

A limit of the output check: the groups of one pattern differ only in a few constants. A program in which zustc
mixed up two groups' items (as in 3) can still print the right checksums. The probes above were made by changing one
group by hand. `generics` now makes its u32 impl differ per group, so taking another group's impl changes its output.

## What was checked

- Every pattern and `mix`, at N = 1, 2, 3 and 4, with and without an edit: zustc's output is rustc's.
- `tests/bench_patterns.rs`, and the three probes in `tests/driver2.rs`.
- `cargo test --release --workspace`, `bench/rust_ui.py --check`.
- KPIs: the `KPI` workflow's comment on the pull request has the change against the base. `patterns check` and
  `patterns edit` are new, so the base has no value for them.
