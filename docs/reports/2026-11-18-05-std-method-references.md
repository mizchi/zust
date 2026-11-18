# Closing the gap in references of std methods (2026-11-18)

Date: 2026-11-18

In the zustc workspace, compared the references of 20 commonly used std methods against rust-analyzer, one by one
(starting from calls in `navigate.rs`).

| Method | Before (zustc / rust-analyzer) | Now |
|---|---|---|
| `Option::map` | 214 / 246 | 266 / 266 |
| `Iterator::map` | 282 / 342 | 349 / 349 |
| `collect` / `push` / `filter` / `starts_with` / `clone` / `is_empty` / `len` / `as_str` / `contains` / `get` / `insert` / `trim` / `find` / `first` / `extend` | Differences of a few to a dozen or so | All the same |
| `ToString::to_string` | 0 / 607 | 605 / 613 |
| `Option::unwrap` / `<[T]>::iter` | 341 / 346, 812 / 822 | 345 / 346, 831 / 832 |

Main fixes:

- **Name resolution**
  - For a method that exists on both the receiver type and its Deref target, pick the one closer to the receiver (`String::len` and `str::len`). Before, there were 2 and no answer was given.
  - Look at the Deref target before traits with blanket impls (`as_ref` on `RefMut<Option<T>>` is Option's).
  - If both a trait method declaration and its blanket impl (`impl<T: Display> ToString for T`) are candidates, pick the declaration.
  - When a const or function with the same name also exists in another file or crate, pick the one in the same file or same crate (`const PRELUDE`, the test `fn run`).
  - Fixed enum variants of an alias from `use .. as PTy` being mistaken for a struct with the same name (`Path`).
- **Types**
  - Elements of nested adapters (`Peekable<Skip<Args>>`).
  - Receivers of type aliases (`type Cur = Option<..>`).
  - Projections such as `<str as ToOwned>::Owned`.
  - `I::Output` of `get(i)`.
  - Tuple struct fields (`self.0`).
  - Functions taking `&mut T` (`mem::take`).
  - Passing a function name: `and_then(str_lit)`.
  - `Some(e)`.
  - Closures whose body is `if` / `match`.
  - `&|t: &str| ..`.
  - Assignment after `let x;`.
  - Fixed qualified types (`&std::cell::RefCell<..>`) being mistaken for projections and discarded.
- **Patterns**
  - Bindings in a match arm that follows an arm whose body ends with `,` without braces (`"-O" => o.opt = true, s if ..`).
  - Match arms with `==` in the guard.
  - Bindings in the guard of `matches!(e, P(g) if g..)`.
- **Library index**
  - Kept the shape of tuple variant fields in the definition, so it is known even for libraries whose text was discarded (proc_macro2's `TokenTree::Literal(l)`).
  - Also kept hints for the result type of `macro_rules!` in the definition (`json!` → `json_internal!` → `Value`).

Still different:

- The type of `&*ptr`.
- The associated const `usize::BITS`.
- `e.to_string()` on a closure parameter (`io::Error` in `map_err(|e| ..)`).
- One place where rust-analyzer goes to the `to_string` on the impl side (the specialization in string.rs).

The match average of `python3 bench/lsp_parity.py` is the same as last time or higher:

| Target | zustc | rust-analyzer |
|---|---|---|
| synth | 99.7% | 99.1% |
| fixture | 97.3% | 95.6% |
| zustc | 96.3% | 95.5% |
| ide | 98.1% (+2.8 pts) | 97.9% |

references p50 is 4.2 ms for zustc and 24.1 ms for rust-analyzer. Adding the index of dependency crates raised memory to 66 MB (rust-analyzer: 2.4 GB).
