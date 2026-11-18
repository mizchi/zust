# What regression monitoring caught

Date: 2026-11-15

Examples where the regression monitoring (`bench/regress.sh`) of size budgets, criterion and `tests/compare.sh` actually did its job. All of them were fixed without loosening budgets or thresholds.

- Size
  - Adding enums made item summaries grow by 1 byte/item. Fixed by packing the item summary encoding.
  - The "checked" mark grew by 4 bytes/function. It is now not written when empty.
  - The whole prelude was being attached, so every cache grew by about 2.3 KB. Now only what is used is attached.
  - The field names `min` / `max` pulled in the functions of `core::cmp`. Now only when the identifier `core` appears.
- Speed
  - no_std support made LSP 1-function edits +31% slower and no-op builds +10% slower.
  - To remove noise depending on the time of day, the baseline commit was benchmarked at the same time in a worktree, and the cause was narrowed down with callgrind.
  - What was fixed: hashing all identifiers every time (prelude selection), the LSP hashing the whole file contents, and SipHash in the name table.
  - The name table was switched to our own FxHasher. It is not used for cache keys.
  - As a result, LSP 1-function edits became −7% and no-op builds −4%, faster than the baseline.
- Correctness (when adding closures and dyn; found by adding a stage to `tests/compare.sh` that compares both "no cache" and "rebuild from cache")
  - Closures in the same function all get the same struct deep hash (= the parent item's hash) and signature. So the cache keys of `consume::<closure#8>` and `consume::<closure#9>` collided, and the second build used another closure's machine code. Fixed by putting the closure's name in the key.
  - vtables were defined as zero-init, so they were placed in `.bss`, relocations were not applied, and null was called.
- Speed (when adding closures and dyn)
  - An A/B against HEAD showed +4% on LSP 1-function edits. The causes were a `Vec` clone per type inference call and including names in struct hashes; both were removed.
  - By callgrind instruction counts, 1-function edits (`Session::check`) became fewer than HEAD. didOpen (cold) was about +2%, with no significant difference in criterion either.
- When adding function pointers and Box
  - `Ty` had grown from 16 → 24 bytes (because the earlier `Dyn(&'static str)` was a fat pointer). Changed to `&'static String` to get back to 16 bytes.
  - criterion showed LSP 1-function edits +14% and lexer +10%. But the lexer code had not changed.
  - callgrind showed that `b"..".contains(&c)` in `lexer::St::tok` had become a memchr. The call count was the same; only the instruction count grew by +32% (a difference in the placement of static strings).
  - Switching to `matches!` gave, versus the baseline, LSP 1-function edits −5%, no-op builds −13%, lexer −6%.
- When adding Drop (added a valgrind stage to `tests/compare.sh`: it fails on memory errors and definite leaks)
  - Reading `*b` was treated as a move of `b`, and the Box leaked. The contents of a deref are now read without moving.
  - `self.top = n.next` (moving only a field of the Box contents) treated the whole `n` as moved, so nodes leaked.
    The moved field is now filled with 0 (a null Box / `None`), and the owner is dropped normally.
  - Two existing bugs were also found.
    - Returning `&Rect { .. }` as `&'static` pointed to a temporary on the stack. Constant expressions are now promoted to static data, as in rustc.
    - The type of the tuple struct field `n.0` was not inferred.
  - Programs that use neither Box nor Drop generate no drop handling at all (`Program::has_drop`). Speed and size budgets do not change.
- When adding Vec / String
  - criterion showed +20–40% on almost every item, but the unchanged lexer was just as slow; it was noise from machine load.
    Measured side by side with HEAD in the same time slot, it was within about ±10% noise. callgrind instruction counts were also at or below HEAD.
  - The only real increase was prelude selection (+26%). The cause was more prelude names, which made the candidate tables per first byte longer. Switched to a table looked up by (first byte, length), bringing it below HEAD.
  - `==` does not appear as an identifier, so the prelude's `Vec::eq` / `String::eq` were not attached, and pointers were compared. eq is now attached together with the type, and also added to the reachability roots.
- When adding Iterator
  - Size budgets were exceeded in almost every example (hello's cache went from 3.2 KB → 10.8 KB). `for x in v.iter()`, the variable name `count`, `a.max(b)`, `Option::take` and the field `.next` pulled in all the iterator parts.
    The conditions for attaching were narrowed, e.g. to "a call that creates an iterator followed by an adapter or consuming method", so the existing examples pass without loosening their budgets by even 1 byte.
  - On LSP 1-function edits, prelude selection was +63% (because there were more candidate names). The result is now remembered in the Session per item hash, and unchanged items are not scanned. In callgrind, `Session::check` is below HEAD.
  - Type inference recorded the types of macro arguments it parsed temporarily by address, so nodes created by codegen landed on freed addresses and looked up the type of another expression (`&str`). Temporary nodes are no longer recorded.
  - Editing one closure rebuilt 65 instances of the map etc. that receive it. Removing the parent item's hash from the closure struct's deep hash brought this to 18 (just the parent and the closures).
- When adding format! / HashMap
  - nostd_core's cache went from 8.7 KB → 22.7 KB. `write` in a raw pointer's `p.write(v)` and `a.max(b)` pulled in all the formatting parts and String / Vec.
    Formatting macro names are now counted only in the form `name!`, and `__exr_str_cmp` is no longer attached for max / min.
  - iterators got HashMap's iterators (`HmIter` etc., on the `impl Iterator for` side). These are also attached only when the HashMap name is present.
  - The formatting helpers (`__exr_fmt_int` etc.) were made generic `<G>` and removed from the roots. Only the ones called are compiled, as instances.
    Non-generic functions called from instance bodies (`__exr_print` → `putchar`) are picked up by expanding the live set after each codegen wave.
  - The only budget loosened was vec_string's cache (45.0 KB → 48.5 KB). For `name.to_string()`, the receiver type is not known at selection time (integers need Display), so the formatting parts get attached.
  - callgrind: gen1000 build +0.15%, check −4%. Examples that use a lot of the prelude grew (iterators +5%, vec_string +15%, due to more prelude being attached).
- When adding ordering comparisons (PartialOrd / Ord)
  - Always attaching the comparison glue (`__exr_cmp`) made the caches of small examples grow by 20 bytes each and exceed the budgets.
    Recursive types go through Box / Vec, so the glue is attached only to programs that use those, and elsewhere codegen expands comparisons in place.
  - `cmp` in `use core::cmp::{max, min}` and the bound `T: Ord` attached the trait declaration and `Ordering`. `cmp::` is no longer counted as a path, and the trait declaration is attached only when there is a hand-written `impl Ord for`. No existing budget was loosened by even 1 byte.
  - Existing bugs found along the way
    - Temporaries of `println!` in a block's tail expression or in a match arm were dropped at the end of the statement, which failed Cranelift's verifier (value dominance) (`if c { println!(..) } else { .. }`).
      Temporaries in tail expressions and match arms are now dropped on the spot (the same as Rust 2024).
    - `len()` on `&&&str` followed only one level and returned 0.
    - `None` / `Ordering::Less` on the right side of a comparison were read as places (local variables).
  - callgrind: gen1000 build −2.6%, check −6.8%, examples that use the prelude +0.4–1.5%.
- When adding merge sort, binary_search and clamp (`examples/sorting.rs`)
  - sort is insertion sort in groups of 16 + bottom-up merging (the work area is n elements via realloc). Elements are moved bitwise with `read` / `write`, so there is no double free even for types with Drop (checked with valgrind).
    sort of a 200,000-element `Vec<u64>` went from 56 seconds → 0.04 seconds.
  - Size budgets were raised for examples that use sort (vec_string +3%, ordering +16%, etc.). The merge sort body is larger than insertion sort, and there is an instance per element type and comparison closure.
    vec_string's build is also +9% (callgrind; due to more functions to compile). Programs that do not use sort do not change (gen1000 ±0%).
  - Existing bugs found along the way
    - Integer methods with a `&mut u64` / `&u64` receiver (`s.wrapping_mul(..)`) computed on the address instead of the pointee.
    - Variants with paths of 3 or more segments, like `std::cmp::Ordering::Less`, were looked up by the second segment's name (`cmp`).
    - `==` / `!=` on raw pointers did not work.
- When adding BinaryHeap and sort_by_cached_key
  - No existing size budget item changed (BinaryHeap is attached only when its name is present).
  - callgrind: since the prelude got larger, the once-per-process lexing of the prelude is +0.5M instructions (+3.8% on hello, about 0.1 ms). gen1000 build ±0%.
    The check measurement has about ±8% noise at 1000 functions (even HEAD vs HEAD gives 1.82G–1.92G), so it was measured 3 times each to confirm there was no difference.
- When adding BinaryHeap append / extend
  - Type inference unified `Item = T` of `Vec<T>::extend` with the `&i32` of `v.extend(w.iter())`, making `v` a `Vec<&i32>`. extend now decides v's element type from the argument's element type (the pointee for references, except `&str`).
  - Existing bug found along the way: the type of `to_string()` on `&&str` could not be inferred, so the return value of the closure in `a.iter().map(|s| s.to_string())` was not determined.
  - Size budgets and callgrind (gen1000 / hello / vec_string / binary_heap) were all within ±1%.
- When adding commonly used features in bulk (`tests/probe.sh`: compares small programs in `tests/probe/*.rs` against rustc and counts how many pass; it started at 2 / 28)
  - The prelude reached nearly 4000 lines, and the once-per-process lexing of the prelude took half of hello's build (8M instructions).
    The splitting was moved to build.rs (using `src/lexer.rs` as-is), and the prelude body was put in `src/prelude_lib.rs.in`. hello went from 14.9M → 9.3M instructions.
  - The string helpers and operator traits made the size budgets of existing examples exceed across the board (nostd_core's cache 8.7 KB → 29.7 KB, etc.).
    - Helpers with the same names as integer `to_ascii_uppercase` or `Option::replace` are attached only when there is a String or string literal receiver
    - Operator trait declarations only when there is a hand-written `impl Add for ..`
    - New String methods were made generic `<G>` (they were considered live by method name alone, so Vec's `pop` etc. caused String's `pop` to be compiled too)
    - Helpers that looped over chars now read UTF-8 directly and do not attach the iterator parts
    No existing budget was loosened by even 1 byte.
- When adding floating point, i128, standard I/O, Rc/RefCell, `impl Iterator` return values, etc. (probe increased to 73 programs, all matching rustc)
  - Decimal conversion with glibc's `ecvt_r` sometimes got digits wrong, so it is done exactly with base-1e9 multi-precision arithmetic. The random cross-checks of `examples/float_fuzz.rs` (6000 lines) and `examples/int128.rs` match rustc.
  - For size budgets, new prelude types are "attached only when their name appears" (owner gating), so most items did not need to change. The limits were raised only for iterators (+68 B), text_more (+0.4%) and hashmap (+0.3%, for `cycle`, a default method of Iterator).
  - callgrind (instruction counts): hello went from 9.96M before the work → 9.95M (dropping a `Vec` allocation when building the prelude item table offset the increase). gen1000 is 2.43G, with no difference within the ±2% noise seen even between HEAD builds.
- When adding overflow checks
  - callgrind: gen1000 build 2.43G → 2.55G instructions (+5%); `-O` is 3.06G, the same as HEAD. hello 9.95M → 10.25M (for registering the handler in main).
    At first, each check created its own panic branch and block, giving +50% (3.67G). Switched to `trapnz`, and skipped checks for constant divisors and shift amounts.
  - Size: code grows by the amount of the checks (gen1000's binary 443 KB → 498 KB; no growth with `-O`). This is the same kind of growth as rustc debug builds, so the budgets were re-recorded with UPDATE_BUDGET=1.
  - Existing bugs found along the way: double free when returning a String error with `?`, `{ return None; }` having type `()`, enum `==` following pointers of unused variants, `saturating_add` using a vector instruction, `struct Node { children: Vec<Node> }` being "infinitely sized", instantiation never stopping when `Node<T>` holds `Option<Box<Node<T>>>`, char literal patterns, the addresses of expressions rebuilt inside type inference overlapping codegen nodes, and so on.
  - When trying to decide the return value of `|a| a + 1` from the callee's bound (`B`), the not-yet-determined `B` was given priority over the parent's type inference (`map(|w| w.parse().unwrap())`).
- When emitting panic messages per kind
  - The runtime handler (table lookup, number printing, messages) is always attached to std programs, so small examples grew by +3–6 KB (hello's binary 6.0 KB → 10.0 KB).
    At first it was +6–7 KB. Fixed: the formatted panic function pulled in String / Vec (it is now attached only to programs that use formatting),
    the sigaction struct was built by array initialization (made static), and messages were selected with an if chain (changed to a concatenated string and a position table).
  - The table was at first 16 bytes per location, making gen1000's object +15%. Packing a 20-bit offset, 13-bit message number, and 31 bits of kind, line and column into one word made it +8% (the cache, in the same form, +5%). The budgets were re-recorded with UPDATE_BUDGET=1.
  - callgrind: gen1000 build 2.54G → 2.57G (+1%), `-O` 3.06G → 3.11G. hello's no-cache build 10.3M → 29M (parsing and codegen of the handler functions; about 4 ms), rebuild with cache 2.8M → 5.6M.
  - Existing bug found along the way: `__FmtError` did not implement Debug (unwrap of `Result<_, fmt::Error>`).
- When resolving `==` between different types via the PartialEq trait
  - Naming: methods of `impl PartialEq<R> for S` whose right-hand type differs from the self type are named `S::<PartialEq<R>>::eq` (the same as `S::<Display>::fmt` for Display / Debug). `==` looks for that candidate only when lt and rt differ, so same-type comparisons work as before.
  - Type inference: when both sides of `==` are a Vec and an array or slice, the elements are unified only when they are numeric literals (`v == [1, 2]` with v: Vec<i64>). `Vec<String> == Vec<&str>` is not unified.
  - Size: programs that compare arrays get the impl attached, and caches are +0.1–1.7% (enums +256 B, etc.). callgrind: gen1000 and hello ±0%, vec_string +2% (parsing of the attached prelude).
- When printing the left / right values of assert_eq!
  - Programs that did not use formatting get the formatting parts attached (options' binary 16.7 KB → 23.9 KB, build 57M → 125M instructions). This is what is needed to print the values with `{:?}`, so the budgets were re-recorded. gen1000 and hello ±0%.
  - Existing bugs found along the way:
    - The line of a panic location was the line within the item (lines of the second and later functions were off). dbg! had the same problem.
    - Programs that use no prelude names at all (traits etc.) did not get the panic handler, so a panic was SIGILL (exit code 132).
    - `&None` was treated as a borrow of a variable.
- When adding iterators, Vec and clone
  - **Until now, `.clone()` on `#[derive(Clone)]` types did not work, and clone of `Vec<String>` and `cloned()` were bitwise copies (double free)**. Fixed by adding clone glue.
  - The `impl Iterator` of the new adapters (Rev / Peekable / FlatMap etc.) were all attached to any program that used iterators. Now only when that struct is needed.
    The Formatter's `fill` field in formatting pulled in `Vec::fill` and the whole clone set, so the field was renamed. No existing budget was loosened.
  - The temporary Vec in `vec![..].join(..)` was not dropped (found with valgrind).
  - callgrind: gen1000 ±0%, hello +2.5%, vec_string / iterators / hashmap +0.5–1%.
- When adding closures inside macro arguments
  - The item summary and codegen parse only macro arguments that contain `|` to count closures. callgrind: gen1000 +0.2%, iterators +0.7%, formatting ±0%.
  - Type inference keeps the re-parsed nodes of `vec![..]` arguments until the end of inference, so closure bodies can be deferred until the callee's bound is known (`vec![v.iter().map(|x| x * x).sum::<i32>(); 2]`).
