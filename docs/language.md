# Supported language subset

zustc compiles a subset of Rust. Programs that use std (Vec / String / HashMap / formatting / iterators / Box /
Rc and so on) run on zustc's built-in prelude (`crates/syntax/src/prelude_lib.rs.in`; it is not the real std).
`#![no_std]` programs are also supported.

Correctness is checked by also building the same program with rustc and comparing the results (stdout and exit code)
(`tests/compare.sh` for `examples/*.rs`, `tests/probe.sh` for `tests/probe/*.rs`). For no_std programs, the rustc side is built with libcore only
(`-C panic=abort`).

**Important premise**: there is no borrow checking, mutability checking, move checking, or match exhaustiveness checking (see "Type inference and checking" below). It is a tool
for building programs that already pass rustc quickly, not a replacement for rustc in checking correctness.

## Language

- Items:
  - `fn`
  - `extern "C"` declarations and definitions
  - `struct`: named / tuple / unit
  - `enum`: unit / tuple / struct variants, and discriminants
  - `const`, `static`, `static mut`
  - `use` (names are resolved by the last segment)
  - Crate attributes (`#![no_std]` and so on)
- impl and trait:
  - Inherent `impl`
  - `trait` (required methods and default methods)
  - `impl Trait for S`: struct / enum / primitive types (`impl Tr for u32`)
  - `#[derive(Clone, Copy, PartialEq, ..)]`
- Generics:
  - Functions (`<T: A + B>`, `where`, turbofish)
  - struct / enum (`Pair<T>`, `Option<T>`)
  - `impl<T> X<T>`, and self type patterns such as `impl<T> Option<&T>`
  - In all cases, type arguments are inferred at the call site and monomorphized. Lifetimes are erased
  - `impl Trait` in argument position (treated as an anonymous type parameter)
- Closures (`examples/closures.rs`):
  - Captures: by reference (default) and `move` (by value). Nested closures, and bodies containing `self` or method calls
  - `F: Fn(A) -> B` / `FnMut` / `FnOnce` bounds, `impl Fn(A) -> B` arguments, passing `&F` / `&mut F`
  - `impl Fn(A) -> B` in return position (when the tail is a `move` closure that captures only arguments)
  - Stored in struct fields (`struct Button<F: Fn(i64) -> i64> { on_click: F }`, `(self.on_click)(x)`)
  - Parameter and return types are decided in this order: annotation → bound of the callee → usage (type inference of the parent) → type inference of the body
- `dyn Trait` (`examples/dyn_trait.rs`):
  - `&dyn Trait` / `&mut dyn Trait`. Implicit conversion from `&T` (arguments, let, fields, arrays, return values) and `as &dyn Trait`
  - Calls through the vtable to default methods, generic impls (`impl<T> Tr for W<T>`), and impls on primitive types
  - `&dyn Fn(A) -> B` (calls a closure through the vtable)
  - Auto traits: `dyn Trait + Send` / `+ Sync` keep them, as rustc's. A value becomes one only if its type has them
    (E0277); a plain `dyn Trait` is neither Send nor Sync unless its trait has them as supertraits (`trait Tr: Send`);
    a coercion may drop them (`Box<dyn Tr + Send>` → `Box<dyn Tr>`), never add them (E0308)
- Function pointers (`examples/fn_ptr_box.rs`):
  - The `fn(A, B) -> R` type. Using function items as values (arguments, return values, let, struct fields, arrays), `(op.f)(a, b)`
  - Passing a function to `F: Fn(A) -> B` (`apply(double, 5)`, `o.map(double)`)
  - For generic functions, type arguments are decided by turbofish (`id::<i64>`) or by the expected type (`let f: fn(i64) -> i64 = id;`)
  - Converting to `&dyn Fn` / `Box<dyn Fn>` (up to 3 arguments; the prelude's trampoline function `__exr_fnptrN` goes into the vtable)
- `Box<T>` (`examples/fn_ptr_box.rs`):
  - `Box::new(x)` places the value on the heap with libc `malloc`, and drop calls `free`
  - `Box<dyn Trait>` / `Box<dyn Fn(A) -> B>`, auto-deref, `*b`, `&Box<T>` → `&T`, and recursive types such as `Option<Box<Node>>`
- `Vec<T>` / `String` (`examples/vec_string.rs`): written in Rust in the prelude (`crates/syntax/src/prelude_lib.rs.in`). They use libc `realloc` / `free`, and Drop drops the elements before freeing
  - `Vec`: `new` / `with_capacity` / `push` / `pop` / `len` / `is_empty` / `capacity` / `get(_mut)` / `first` / `last` / `insert` / `remove` / `swap_remove` / `swap` / `reverse` / `truncate` / `clear` / `append` / `as_slice` / `as_mut_slice` / `extend_from_slice` / `clone` (`T: Copy`) / `contains` / `==` (`T: PartialEq`) / `sort` (`T: Ord`, insertion sort)
  - `vec![a, b]` / `vec![x; n]`, indexing `v[i]` (panics when out of range), `&v[a..b]`, `for x in &v` / `v.iter_mut()` / `v` (iterating by value)
  - Implicit conversions `&Vec<T>` → `&[T]` and `&String` → `&str` (also to generic `&[T]` arguments)
  - `String`: `new` / `from` / `with_capacity` / `push_str` / `push(char)` (encoded as UTF-8) / `len` / `is_empty` / `as_str` / `as_bytes` / `clear` / `truncate` / `clone` / `==` (between `String`s and with `&str`), `"..".to_string()` / `to_owned()`
  - `==` on `&str` / `&[T]` compares contents
  - Vec / String methods are attached from the prelude only when the type name appears in the code (having a `push` on your own struct is not enough)
- Iterator (`examples/iterators.rs`): the prelude has `trait Iterator { type Item; fn next(..); .. }` and the adapters written in Rust
  - Associated types: `type Item = u32;` (impl) and `Self::Item` / `I::Item` (signatures); type parameters are solved from the constraints `I: Iterator<Item = T>` / `where Self: Iterator<Item = &T>`
  - Your own `impl Iterator for S`, `for x in it` (turned into `while let Some(x) = it.next()`)
  - Producers: `v.iter()` / `iter_mut()` / `into_iter()` (Vec, arrays, slices), `(a..b)` / `(a..=b)` as values, `s.chars()` (decodes UTF-8)
  - Adapters: `map` / `filter` / `filter_map` / `enumerate` / `zip` / `take` / `skip` / `take_while` / `chain` / `copied` / `cloned`
  - Consumers: `collect` (`Vec<_>` / `String`, including the `_` in `collect::<Vec<_>>()` and `let v: Vec<_> = ..`) / `sum` / `product` / `count` / `fold` / `for_each` / `any` / `all` / `find` / `position` / `last` / `nth` / `max` / `min`
  - Closure parameter types are decided by solving the callee's bound (`F: FnMut(Self::Item) -> B`) down to associated types. Type inference also follows associated types (the range in `let s: i64 = (1..=10).sum();` is i64)
  - The prelude's iterator parts are attached only when there is code that creates an iterator and calls adapters or consumers (`v.iter().map(` / `(0..n).sum(` / `impl Iterator` / `.chars()`). They are not attached for just `for x in v.iter()`, just `Option::map`, or just a variable named `count`
- Formatting (`examples/formatting.rs`): `format!` / `print!` / `println!` / `eprint!` / `eprintln!` / `write!` / `writeln!` (to `String` and `fmt::Formatter`)
  - `{}` / `{:?}` / `{:#?}`, positional and named arguments, `{x}` captures, `{{` / `}}`
  - Width, alignment and fill (`{:>8}` / `{:*^9}` / `{:08}` / `{:>w$}`), `+`, `#`, precision (truncation of strings), `x` / `X` / `b` / `o`
  - `impl fmt::Display for S` (`fmt::Result` / `fmt::Formatter<'_>` / `f.pad`) and `x.to_string()`
  - `#[derive(Debug)]` (structs, tuple structs, unit structs, enums, generics). Debug for integers, bool, char, `&str`, String, Vec, arrays, slices, tuples, Option, Result, HashMap
  - The per-type Display / Debug bodies are generated by codegen (`__exr_display::<T>` / `__exr_debug::<T>`). The prelude has only helpers written in Rust
- HashMap (`examples/hashmap.rs`, `examples/text_more.rs`): written in Rust in the prelude (open addressing, linear probing, FNV-1a)
  - `new` / `with_capacity` / `insert` / `get` / `get_mut` / `remove` / `contains_key` / `len` / `is_empty` / `clear` / `m[&k]`
  - `entry(k).or_insert(v)` / `or_insert_with` / `and_modify`
  - `iter` / `iter_mut` / `keys` / `values` / `values_mut` / `into_iter` / `for (k, v) in &m`, `collect::<HashMap<_, _>>()`
  - Keys: integers, bool, char, `&str`, String, and `#[derive(Hash)]` structs / enums / tuples (codegen generates a hash function per type)
  - Iteration order differs from rustc (rustc's is also random, so the examples sort the keys before printing)
- Strings and char (`examples/strings_chars.rs`): if the prelude has `__exr_str_<name>` / `__exr_char_<name>`,
  `s.name(..)` / `c.name(..)` calls it (type inference also uses that signature; a char can be passed to a &str parameter)
  - `to_uppercase` / `to_lowercase` / `replace` / `replacen` / `repeat` / `split("..")` / `split_once` / `strip_prefix` / `strip_suffix` /
    `trim_start` / `trim_end` / `rfind` / `char_indices` / `bytes` / `eq_ignore_ascii_case` / `parse::<integer>()` (Display / Debug of `ParseIntError` are also the same as std)
  - String's `insert` / `insert_str` / `remove` / `pop` / `retain` / `+` / `+=`, `&s[a..b]` (panics if not on a char boundary)
  - char's `is_alphabetic` / `is_numeric` / `is_alphanumeric` / `is_whitespace` / `is_uppercase` / `is_lowercase` / `to_digit` / `is_digit` /
    `to_uppercase` / `to_lowercase` / `len_utf8`, `char::from_digit` / `char::from_u32` / `char::from(u8)`.
    There are no Unicode tables, so it approximates within the ranges of ASCII, Latin-1, Greek, Cyrillic, kana, kanji and Hangul
- More iterators (`examples/iter_vec_more.rs`): `rev` (on Vec, arrays, ranges, chars, and after map / filter / copied / chain; instead of DoubleEndedIterator, each type has `next_back`) /
  `step_by` / `peekable` (`peek` / `next_if`) / `skip_while` / `scan` / `inspect` / `flat_map` / `flatten` (contents are Vec, Option or iterators)
- More Vec / slice: `retain` / `retain_mut` / `dedup` / `dedup_by_key` / `drain(range)` / `split_off` / `resize` / `fill` / `first_mut` / `last_mut` /
  `windows` / `chunks` / `to_vec` / `join` / `concat` (elements are `&str` or String)
- More Option / Result: `expect` / `zip` / `map_or_else` / `or_else` / `and` / `get_or_insert_with` / `insert` / `is_none_or` / `is_ok_and` / `is_err_and` / `map_or`
- clone (`examples/clone_derive.rs`): `#[derive(Clone)]` structs / enums / generics, Box, tuples, arrays, Option, Result, and hand-written `impl Clone`.
  The body is generated per type by codegen (`__exr_clone::<T>`; fields that need no drop are copied, others are cloned field by field)
- Default (`examples/defaults.rs`): `#[derive(Default)]`, hand-written `impl Default`, `Default::default()` / `T::default()` / `i32::default()` / `Vec::default()`,
  struct update syntax `S { a: 1, ..Default::default() }` / `..base`, `unwrap_or_default`, `std::mem::take` / `swap` / `replace` (also for non-Copy types).
  The body is generated per type by codegen (`__exr_default::<T>`: fills with 0, and calls the hand-written impl for fields that have one)
- Collections (`examples/collections.rs`): `HashSet` (on top of HashMap<T, ()>; union, intersection, difference, collect), `BTreeMap` / `BTreeSet` (a Vec kept in key order, with binary search;
  iteration order, `entry().or_insert` / `or_default`, `first_key_value`, `pop_first` and `m[&k]` are the same as std), `VecDeque` (ring buffer; `d[i]`, push / pop at both ends).
  `get("k")` on a map with `&str` / String keys can look up by `&str`, the same as std's Borrow
- Threads (`tests/threads/`): `thread::spawn` / `join` / `is_finished` / `yield_now` / `sleep` / `current` / `park` /
  `park_timeout` / `Thread::unpark`, `thread::Builder` (`name`, `stack_size`, `spawn`), `mpsc` channels (`channel`, `sync_channel`;
  `recv_timeout`, a `Receiver`'s `into_iter`), `Mutex`, `Condvar`, `RwLock`, `Barrier`, `Once`, `OnceLock`,
  atomics, `Arc`, `thread_local!`. They are OS threads (pthreads, 2 MiB stacks by default), run in parallel, as std's: the atomics
  and `Arc`'s count are atomic instructions, and the locks wait on futexes with std's state machines. A wait that
  nothing can end waits forever, as std's. A panic in a spawned thread prints `thread '<unnamed>' panicked at ..` (a Builder's name in its place), drops
  that thread's `thread_local!` values and ends it, and its `join` gives `Err` (`Any { .. }`). There is no unwinding,
  so its other values are not dropped and a Mutex or RwLock it holds stays locked (no poisoning; a `Once` it was
  running is poisoned, as std's). A panic in a `thread_local!` value's drop, while the thread ends, aborts the program
  instead (std's "thread local panicked on drop"): `join` cannot recover from that one
- `impl Index` / `IndexMut`: `x[i]` becomes `*x.index_mut(i)` if x is a struct with `index` / `index_mut`
- Associated constants (`examples/assoc_consts.rs`): `impl S { const N: usize = 4; }` / trait `const K: u32;` (also with default values). `S::N` / `Self::N` / `T::N` (type arguments), also in array lengths
- Modules (`examples/modules.rs`): `mod m { .. }` within one file (nested, `pub(crate)`, `use self:: / super:: / crate::`, `m::f()` / `m::S` / `m::N`).
  Before lexing, `mod m {` and its matching `}` are replaced with whitespace to flatten them (positions and lines do not change), and name resolution skips the leading module names of a path.
  `#[cfg(test)] mod tests { .. }` is removed
- `macro_rules!` (`examples/macros.rs`): expanded as text before lexing (an expansion fits on 1 line, so later line numbers do not change).
  Fragments expr / ident / ty / literal / tt / block and so on, repetitions `$(..),*` / `$(..)+` / `$(..)?`, recursion, and macros that produce items.
  Expression fragments are embedded wrapped in `( .. )`, so the precedence of `square!(1 + 2)` is the same as Rust
- Operator overloading: `impl Add / Sub / Mul / Div / Rem / Neg / Not / BitAnd / BitOr / BitXor / AddAssign ..`.
  `a + b` becomes `a.add(b)` if the left side is a struct with an `add` method (also right-hand types such as `impl Mul<i32> for V`)
- `&str` methods: `split(char)` / `split_whitespace` / `lines` / `trim*` / `starts_with` / `ends_with` / `contains` / `find` (argument is `&str` or `char`), comparisons such as `<`, and `sort`
- Ordering comparisons (`examples/ordering.rs`, `examples/ordering_impl.rs`, `examples/ordering_inline.rs`):
  - `<` / `<=` / `>` / `>=` on tuples, arrays, slices, `&str`, String, Vec, Option, Result, and `#[derive(PartialOrd, Ord)]` structs / enums (lexicographic; enums by variant discriminant order)
  - Hand-written `impl PartialOrd` / `impl Ord` (also `impl PartialEq` / `impl Eq for S {}`). `<` and sort call them
    - Also sort / sort_unstable / max / min / clamp / BinaryHeap / BTreeMap / BTreeSet / Reverse / bounds `T: Ord` / `T: Eq`, inside tuples and Option, and impls on enums
    - Elements of `==` on arrays, slices, Vec and references also call the hand-written eq
    - collect into `BTreeSet` / `BTreeMap` keeps the later one when equal items follow each other, same as std (insert keeps the earlier key)
  - `cmp` / `partial_cmp` (also on integers), `Ordering` with `reverse` / `then` / `then_with` / `is_lt` and so on, `std::cmp::Reverse`
  - `max` / `min` (also on Ord structs, tuples, String), `std::cmp::max` / `min`
  - `sort` / `sort_by` / `sort_by_key` / `sort_by_cached_key` / `sort_unstable*` (stable merge sort; Vec, arrays, slices, `v[a..b]`), `binary_search` / `binary_search_by` / `binary_search_by_key` (the position returned when equal values are adjacent is also the same as std), `clamp` (integers and Ord types; panics if `lo > hi`), iterator `max` / `min` / `max_by_key` / `min_by_key` / `max_by` / `min_by`
  - `BinaryHeap` (`examples/binary_heap.rs`): `new` / `with_capacity` / `from(vec)` / `collect` / `push` / `pop` / `peek` / `len` / `is_empty` / `clear` / `iter` / `into_iter` / `into_vec` / `into_sorted_vec` / `{:?}`.
    The sift steps are the same as std, so the order of `iter` / `into_vec` / `{:?}` also matches rustc. Min-heap via `Reverse`
  - `BinaryHeap::append` / `extend` (`examples/binary_heap_extend.rs`): the same `rebuild_tail` as std (rebuild everything if many are added, otherwise sift_up one at a time; a different threshold above 2048 elements), so the order after adding also matches
  - `Vec::extend`. The argument can be a Vec, an array, `&v` / `&[..]`, a range, or an iterator (instead of IntoIterator, codegen inserts `into_iter()` / `iter()`; references to elements are added as Copy)
  - The comparison bodies are generated per type by codegen (`__exr_cmp::<T>`; in programs that do not use Box / Vec, they are expanded inline)
- Drop (`examples/drop.rs`, `examples/drop_more.rs`; the drop order is compared with rustc, and valgrind confirms there are no leaks or double frees):
  - `impl Drop for S` (also generic impls), `drop(x)`
  - Drops in reverse declaration order at the end of a scope. This covers scopes exited by return / `?` / break / continue, expression statements that discard a value, `let _ = e`, and old values overwritten by assignment
  - Tracks moves: reading as a value, `self` taken by value, `move` captures, patterns that bind by value (`if let Some(b) = o`), `for x in arr`
  - Temporaries (`make().area()`, `&Box::new(..)`, match scrutinees) are dropped at the end of the statement. Temporaries in a let initializer are dropped at the end of the let, unless the initializer extends them by its syntax (`&temp()`, also inside a tuple, array, cast, struct literal or block tail) or the bound value's type holds a reference; then they last to the end of the block. Those of an `if let` scrutinee are dropped before its `else` block, as in Rust 2024, and after its then block (unless the value may hold a reference, when they last to the end of the statement)
  - A move of only a field (`self.top = n.next`) fills the original field with 0 and drops the owner
- Types:
  - Integers (`i8`–`i64`, `u8`–`u64`, `isize`, `usize`), `bool`, `char`, `()`, `!`
  - `&T`, `&mut T`, `*const T`, `*mut T`
  - `[T; N]` (N is a literal or a const), `&[T]`, `&mut [T]`, `&str`
  - Tuples, structs, enums
  - C type aliases such as `core::ffi::c_int`
- Expressions:
  - Arithmetic, bitwise operations, comparisons, short-circuit evaluation, `as`
    - Casts: integer ⇔ char / bool, reference → pointer, pointer ⇔ usize, fieldless enum → integer
  - `let` (also destructuring), assignment, compound assignment
    - The left side can be a local variable, a field, `*p`, `a[i]`, or a static
  - `if` / `match` / `if let` / `while let` / `while` / `loop` / `for` (`break` / `continue` / `return`)
    - Labeled `break 'outer` / `continue 'outer`, `break v` (the value of a `loop`), labeled blocks `'a: { .. break 'a v; .. }`
    - `let PAT = e else { return; };`
  - What `for` can iterate over:
    - `a..b`, `a..=b`, `(a..b).rev()`
    - Arrays and slices (`&xs`, `&mut xs`, by value), and those with `.iter()` / `.iter_mut()` / `.copied()` / `.enumerate()` / `.rev()` applied
  - Construction: struct / tuple / enum / array (`[a, b]`, `[x; N]`)
  - Indexing `a[i]` (panics when out of range), `&a[lo..hi]` (also `..` / `lo..` / `..=hi`)
  - Implicit conversion `&[T; N]` → `&[T]`
  - `==` / `!=`: structs / enums / tuples / arrays are compared structurally. A hand-written `eq` is called if there is one
  - Literals: `"..."` (`&'static str`), `b"..."`, `b'x'`, `'x'`, negative literals
  - `?` (Option / Result)
  - Macros: `panic!` / `unreachable!` / `todo!` / `assert!` / `assert_eq!` / `assert_ne!` / `debug_assert*!` / `matches!`
    - Messages are not formatted
    - `assert_eq!` / `assert_ne!` compare as `==` / `!=` (also Option, Vec, String with `&str`, and so on)
    - Closures inside macro arguments also work (`println!("{:?}", v.iter().map(|x| x * 2).collect::<Vec<_>>())`, `examples/closures_in_macros.rs`).
      Closures are identified by the position of their first `|` (so nodes still correspond when codegen re-parses the macro arguments)
- Patterns:
  - `_`, bindings (`ref` / `mut`), `x @ p`, literals, ranges, `|` (bindings are allowed, as in `A(x) | B(x)`)
  - Slice patterns `[a, b]` / `[first, .., last]` / `[x, rest @ ..]` (arrays, slices, `v[..]`)
  - Variants, structs, tuples, `&p`, nesting, guards
  - Default binding modes when matching on a reference
  - Unit variants such as `None`
- Methods:
  - Auto-deref and auto-ref, UFCS (`S::m(&x)` / `Trait::m(&x)` / `<S as Trait>::m(&x)`)
  - Static dispatch. Lookup order: inherent methods → trait impls → default methods
- Floating point `f32` / `f64`:
  - Arithmetic, `%`, comparisons, `as` casts (conversion to integers saturates, same as rustc), literals `1.5` / `1e-3` / `2.0f32`
  - Methods: `sqrt` / `abs` / `floor` / `ceil` / `round` / `trunc` / `powi` / `powf` / `exp` / `ln` / `sin` / `cos` / `atan2` / `hypot` / `mul_add` / `max` / `min` (ignore NaN) / `total_cmp` / `is_nan` / `to_bits` and so on (transcendental functions use libm)
  - Constants: `f64::MAX` / `EPSILON` / `NAN` / `INFINITY` / `consts::PI` and so on. Path calls such as `f64::sqrt(x)`
  - Formatting uses the same digits as rustc: shortest representation (the shortest digits that round-trip to the same value; `{:?}` uses the `1e-7` form when the exponent is -5 or less or 16 or more), `{:.N}` (round-half-to-even from the exact decimal expansion). Decimal conversion is done exactly with base-1e9 multi-precision arithmetic
  - `"1.5".parse::<f64>()` (strtod, `ParseFloatError`)
  - The 6000 lines of `examples/float_fuzz.rs` (Display / Debug / `{:.N}` / parse of random bit patterns) match rustc
- 128-bit integers `i128` / `u128`:
  - Arithmetic, comparisons, shifts and bitwise operations use Cranelift's I128 (arguments and return values use 2 registers, the same as SysV, via `enable_llvm_abi_extensions`)
  - Division, remainder, conversion to and from floating point (rounded once), formatting (`{}` / `{:x}` / `{:b}` / width and so on), and `parse` are prelude functions
  - `checked_*` / `wrapping_*` / `pow` / `count_ones` / `MAX` / `MIN`, HashMap keys, derive
  - The randomized check in `examples/int128.rs` (3000 pairs of arithmetic, shifts and comparisons) matches rustc
- Other:
  - Integer `overflowing_*` / `checked_pow` / `saturating_pow` / `rem_euclid` / `div_euclid` / `next_power_of_two` / `leading_ones` / `isqrt` / `from_str_radix`
  - Calling primitive methods in function form, such as `i32::pow(2, 10)`
  - `From` impls and `.into()` (`from` of the expected type; `String::from` for String, a cast for numbers)
  - `vec![v; n]` with non-Copy elements (cloned and laid out), `concat` of `Vec<Vec<T>>`, `collect::<String>()` with `&str` / `String` elements
  - `partition` / `rposition`, `collect::<Option<Vec<T>>>()` / `collect::<Result<Vec<T>, E>>()`, type inference for method turbofish
  - `String::from_utf8` / `std::str::from_utf8` (validation and the display of `Utf8Error` are the same as std), the `trim_matches` family (pattern is &str / char / closure), `rev()` on `split` / `lines` / `split_whitespace`
  - `Rc` (`clone` / `strong_count` / `ptr_eq`; no weak references), `RefCell` (`borrow` / `borrow_mut` only return references and do not check borrows), `Cell`
  - Searching the `deref` target when a method is not found (`rc.borrow_mut()`). Recursive types such as `struct Node { children: Vec<Node> }` and `Vec<Rc<RefCell<Node>>>`
  - Format `{:e}` / `{:E}` (also with precision), `{:w$}` / `{:.p$}` / `{:.*}` taking width and precision from arguments, combinations of width and type such as `{:12e}`
  - Type inference inside the arguments of format macros such as `println!` (the types of re-parsed nodes are passed to codegen by their source position)
  - Unbounded ranges `(1..)`, passing `char::is_ascii_digit` and similar as function values
  - Using variants imported with `use E::*;` by their short names, derive(PartialEq) on recursive types through `Box` (generates a comparison glue function)
  - `try_into` (between integers; out of range gives `TryFromIntError`), `i64::from(x)`, char ranges `'a'..='e'`
  - `BTreeMap::range` (`a..b` / `a..=b` / `a..`), `entry(..).or_default()` on `HashMap`
  - Type inference of the loop variable in a for through adapters, such as `for x in (0..n).step_by(k)`
  - `Vec`'s `rotate_left` / `rotate_right` / `split_at`, `Iterator::eq`, construction through paths with `std::`, such as `std::cmp::Reverse(x)`
  - Closures that capture and return local variables (`let mut c = 0; move || { c += 1; c }`; the type comes from an annotation or from a literal or String initializer)
  - `From` conversion of errors with `?` (`impl From<ParseIntError> for AppError`)
  - Items written inside functions (`fn` / `struct` / `enum` / `const` / `use`; moved lexically to the end of the file)
  - Long paths such as `std::collections::HashMap::new()`, `VecDeque::from(vec)`, slice `split_at` / `split_at_mut`, `Chars::as_str`
  - Standard I/O: `read_line` / `lines()` / `read_to_string` / `lock()` on `io::stdin()`, `io::stdout()` / `lock()` / `flush()`, `BufWriter::new(..)`, `write!` / `writeln!` to any type with `write_str` (uses libc stdio, so the order does not get mixed up with `print!`)
  - `io::Read` on `&[u8]` and `Cursor`, `io::BufRead` on `&[u8]`, and `BufReader::new(r)` / `with_capacity` over any `Read` (`fill_buf` / `consume` / `read`; `&mut B` is `BufRead` when `B` is), `io::Seek` / `SeekFrom` on a `Cursor`
  - `std::process::exit`, `dbg!`, `for i in 0..` (unbounded range), `rev()` on `char_indices()` / `bytes()`
  - Closures that return types containing closures (`flat_map(|i| (0..i).map(move |j| (i, j)))`, `|a| move |b| a + b`): type inference solves down to the closure's struct type (the parent's type arguments, captures, parameters, return value)
  - `Iterator::cycle`, `as_ref()` / `as_mut()` on `Box<T>`
  - Formatting builders `f.debug_struct(..)` / `debug_tuple` / `debug_list` / `debug_set` / `debug_map`, `f.alternate()` / `width()` / `precision()` / `pad()`
  - `HashMap::from([..])` (also HashSet / BTreeMap / BTreeSet / VecDeque / BinaryHeap), HashMap's `retain` / `drain` / `extend`, `extend` on HashSet / BTreeMap / BTreeSet / String, `[a, b].into_iter()` as a value
  - UFCS `Ord::cmp(&a, &b)` / `PartialOrd::partial_cmp(..)`
  - char literal and range patterns (`'a'..='z'`), string literal patterns (`match s { "add" => .. }`)
  - Parameter patterns (`fn index(&self, (i, j): (usize, usize))`), operator impls on references (`impl Add for &M`)
  - Passing `&Vec` / slices / arrays to parameters that take an `Iterator` (`a.iter().zip(&b)`), methods with a range index as the receiver (`v[a..b].iter()`)
  - User `impl IntoIterator for &Coll` (`for x in &c`), where clauses that bound things other than type parameters (`where C::Item: Debug` / `where Self: Sized`; skipped)
  - `Box<dyn Error>` (also `dyn Error + Send + Sync`; represented by a prelude type that stores the `{}` / `{:?}` strings at the point of conversion; converted with `?` / `Box::new(e)` / `"msg".into()`), `impl std::error::Error for E {}`, `fn main() -> Result<(), E>` (on Err, prints `Error: {:?}` to stderr and exits with code 1)
  - Generic types that contain themselves through `Option<Box<..>>` (`Node<T> { next: Option<Box<Node<T>>> }`)
  - Blanket impls (`impl<T: Display> Describe for T`; the bound is not checked, and when a method is not found, it is looked up for any type)
  - `impl Trait` return values other than closures, such as `-> impl Iterator<Item = T>` (right after the program is assembled, the body is type-inferred and the concrete type is filled in; the solved type goes into the cache key of the caller)
  - Calling a `const fn` from a const initializer, consts at the ends of range patterns (`i32::MIN..=-1`), `?Sized` bounds
  - Const generics `const N: usize` (type parameters of functions, impls and structs)
    - Array length `[T; N]`, `N` as a value (`for i in 0..N`, `[0; N]`, `N * 10`)
    - Arguments `f::<3>()` / `Buf<4>` / `Mat::<C, R>::zero()` / const names `zeros::<K>()`, inference from array arguments (`sum([1, 2, 3])`)
    - The type argument position holds an array type with that length, `[(); n]`, and `N` inside an instance is read from it (the length for a generic type is `CONST_LEN_BASE + i`). Lengths given by expressions (`[T; N * 2]`) are not supported (also not supported in stable rustc)
  - Type arguments on the type segment in `Type::<A>::f()` (also `Vec::<i32>::new()`)
  - `==` / `!=` between different types is resolved through traits: an `impl PartialEq<R> for S` with the right-hand type (including hand-written ones) is chosen by the types of both sides (also with one side's reference removed), and its `eq` is called
    - The prelude has the same combinations as std: `Vec<T>` with `[U; N]` / `&[U; N]` / `&[U]` / `[U]` / `Vec<U>`, `[T]` with `Vec<U>` / `[U; N]`, `[T; N]` with `[U]` / `&[U]` / `[U; N]` (element types may also differ: `Vec<String> == ["a", "b"]`)
    - Arrays and slices as the self type of an impl (`impl .. for [T; N]` / `for [T]`)
    - `a[..2] == [1, 2]` (comparisons of unsized slices compare by reference)
  - `<` / `<=` / `>` / `>=` between different types likewise use `partial_cmp` of `impl PartialOrd<R> for S` (`Meters < Feet`, `Ver < (1, 5)`, `Ver < 2`)
    - If the right side is a literal, tuple or array, the R of a matching shape among the left type's impls becomes the expected type (`(1, 5)` becomes `(u32, u32)`). The same applies to `==`
    - If `partial_cmp` is `None`, all four are false (same as rustc; also for hand-written partial_cmp on the same type)
    - `partial_cmp` on f64 / f32 is `None` for NaN
- panic:
  - If there is a `#[panic_handler]`, it is called (out-of-range access, `unwrap`, `assert!`, `panic!`, overflow checks, and so on)
  - Otherwise it traps. In std programs (not `no_std`), a signal handler for trap (SIGILL) is registered at the start of main,
    and, like a rustc panic, it flushes stdout and exits with code 101
  - stderr gets the location and a per-kind message in the same form as rustc (on the line after `thread 'main' panicked at src/main.rs:line:col:`; the file name is fixed to `src/main.rs`)
    - Overflow, division by 0, `MIN / -1`, shift amount, `panic!` / `unreachable!` / `todo!` / `unimplemented!` (also with format), `assert!` (the expression string or a custom message), `assert_eq!` / `assert_ne!` (the `left` / `right` values with `{:?}`; also custom messages)
    - Out-of-range indexing gives `index out of bounds: the len is N but the index is I`
    - `unwrap` / `expect` on `Option` / `Result` report the caller's location (equivalent to `#[track_caller]`). The `Err` value of `Result::unwrap` is printed with `{:?}` only in programs that use formatting
  - `assert_eq!(a, b)` expands to `let l = &(a); let r = &(b); if !(*l == *r) { panic }`, evaluating a and b only once.
    The right side's type is decided from the left (`assert_eq!(o, None)`). If the left is a numeric literal, the right is evaluated first and the left's type is decided from the right (`assert_eq!(2, n)`).
    Because the values are formatted, std programs that use assert_eq! get the formatting parts attached
  - The line of a location is the line in the file, including the item's start line. The cache of a function with a location written into it remembers the start line, and recompiles if lines are added above it and the lines shift
  - How it works: each trap gets its own trap code, and codegen records "offset within function → kind, line, column, message".
    The driver turns that into a static table `__EXR_TRAP_TABLE` (8 bytes per location), and the SIGILL handler (`SA_SIGINFO`) looks it up by the faulting address.
    The checks themselves stay `trapnz`, so check speed does not change. Only out-of-range indexing, which passes the length and the index, calls `__exr_panic_index` from one cold block per function
- Overflow checks (same as a rustc debug build; not inserted with `-O`):
  - `+` / `-` / `*` (and `+=` and so on), unary `-`, `abs`, `pow`, `<<` / `>>` amounts at or above the type width
  - Division by 0 and `MIN / -1` (checked even with `-O`, same as rustc). Omitted when the divisor or shift amount is a constant
  - `sum` / `product` (follow the caller's setting, same as std: `#[rustc_inherit_overflow_checks]` in the prelude)
  - No checks inside the prelude (it contains arithmetic that wraps on purpose, such as hashing). 128-bit multiplication is not checked
  - Checks are conditional traps (`trapnz`), not branches. Passing the panic location through branches and blocks makes gen1000 compilation +50%; with `trapnz` it is +5%

## Replacement for core (built-in prelude, `crates/syntax/src/prelude_lib.rs.in`)

- `Option<T>`: `is_some` / `is_none` / `unwrap` / `unwrap_or` / `ok_or` / `or` / `xor` / `take` / `replace` / `as_ref` / `as_mut` / `copied` / `cloned`
  - Ones that take closures: `map` / `and_then` / `unwrap_or_else` / `map_or` / `is_some_and` / `filter` / `ok_or_else`
- `Result<T, E>`: `is_ok` / `is_err` / `unwrap` / `unwrap_err` / `unwrap_or` / `ok` / `err`
  - Ones that take closures: `map` / `map_err` / `and_then` / `unwrap_or_else`
- `core::panic::PanicInfo`
- `core::cmp::{min, max}`, `core::mem::{swap, replace, size_of, align_of}`, `core::ptr::{null, null_mut, read_volatile, write_volatile}`
- Built-in methods:
  - Integers: `wrapping_*` / `checked_*` (→ Option) / `saturating_*` / `min` / `max` / `abs` / `abs_diff` / `signum` / `pow` / `count_ones` / `leading_zeros` / `trailing_zeros` / `is_power_of_two` / `rotate_*` / `swap_bytes` / `is_ascii_*` / `to_ascii_*`
  - Associated constants: `u32::MAX` / `MIN` / `BITS`
  - Arrays and slices: `len` / `is_empty` / `get` / `get_mut` / `first` / `last` / `swap` / `contains` / `as_ptr` / `as_mut_ptr`
  - `&str`: `len` / `is_empty` / `as_bytes` / `as_ptr`
  - Raw pointers: `add` / `sub` / `offset` / `is_null` / `read` / `write` / `read_volatile` / `write_volatile` / `cast`

The prelude attaches only the items reachable from identifiers in the user's code. Nothing is added to a program that does not use `Option`, so neither the cache nor the build time grows.

- `core::cmp::max` and similar are kept under `__core_*` names so they do not collide if the user defines a function with the same name.
- They are attached only when `core::` or `use core::` appears in the code.

## Type inference and checking

- Type inference is done by per-function unification (`crates/sema/src/ast_infer.rs`).
  - It also unifies the shapes of references, pointers, arrays, slices, and generic ADTs.
  - It also records the types of literals, constructions and calls, and codegen uses them when the expected type is unknown (the `1` in `Pair { a: 1 }.swap()` is decided as `i64` from a later use).
- There is no borrow checking, mutability checking, move checking, or match exhaustiveness checking.
  - Structs are always copied, on the premise that the meaning does not change for programs that pass rustc.
  - If no match arm matches, it traps.
  - Overflow is checked the same as a rustc debug build (with `-O` it wraps; see "Overflow checks" above).

## Not supported

- async: `async fn` / `async {}` / `.await` are accepted but compiled as plain code. An `async fn` runs its body when called and returns its value (not a future), so a future that is never polled still runs, and `pin!` / `Future::poll` on an async block do not work
- `Box<[T]>`, and Box functions such as `Box::into_raw` (other than `new`)
- `String + &str` / some `Vec` slice methods (other than those written in the prelude; turning `&v` into `&[T]` gives access to the built-in slice methods)
- Iterator limitations: the `DoubleEndedIterator` / `ExactSizeIterator` traits (`rev` only works on prelude types that have next_back); `collect` only into what the prelude has, such as Vec / String / HashMap / HashSet / BTreeMap / `Option<Vec<T>>` / `Result<Vec<T>, E>`
- Associated type limitations: trait associated types only in the forms `Self::X` / `T::X` / `<T as Tr>::X`. Associated types cannot have bounds (`type Item: Copy;`)
- Formatting limitations: `{:p}`, `format_args!`, `fill()` / `align()` on `Formatter`
- Comparison limitations: `partial_cmp` is always `Some` except for floating-point NaN. BinaryHeap's `drain` / `retain` / `peek_mut`. There are no general `IntoIterator` bounds (`extend` has codegen turn the argument into an iterator)
- HashMap limitations: hand-written `impl Hash` for keys
- Drop limitations
  - No drop on panic (no unwinding)
  - If a pattern moves only some fields of a type with `impl Drop`, the remaining fields leak (`b` in `let Pair { a, .. } = p;`)
  - Moving a value captured with `move` out of the closure body (consuming it as FnOnce) is an error
  - Moving a field out of a `Box`'s contents (`let n = boxed; n.val`) leaks the Box's allocation
  - Temporary lifetimes approximate rustc's rules (a value that borrows a temporary dropped at the end of its let is not rejected: E0716)
- Closure limitations
  - Fn / FnMut / FnOnce are not distinguished (turning an `FnMut` closure into `&dyn Fn` is not an error)
  - When returning a closure that captures local variables as `impl Fn`, variables whose type cannot be determined from an annotation or a literal (`let v = Vec::new();` and so on)
- `dyn` limitations: methods that use `Self` or generics (not object safe) cannot be called through dyn. `dyn A + B` is not allowed
- Modules within one file are flattened, so items with the same name in different modules collide (for `mod x;` in a separate file, `x.rs` / `x/mod.rs` is read)
- Items inside functions are moved to the end, so they collide with items of the same name inside other functions. Line numbers in those errors are for the moved location
- `macro_rules!` limitations: no hygiene (expansion is text replacement). `$x:expr` and similar are approximated as the token sequence up to the next delimiter
- `-> impl Trait` return values of generic functions
- `iter().flatten()` on a Vec of references (`into_iter().flatten()` works), traits with type parameters (`From<T>` keeps only the name, and at most one `From` impl into the same type)
- impls on built-in types other than primitives, such as `impl Trait for [T; N]`
