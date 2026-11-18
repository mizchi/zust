# TODO

## Compiler (zustc)

- **Language**
  - Modules, `DoubleEndedIterator` (`rev` as a value), `HashSet` / `BTreeMap`
  - Share monomorphized instances keyed by layout (make `Vec<A>` and `Vec<B>` one; blaze's polymorph)
  - A minimal std of our own (the relax approach)
- **Incremental**
  - Alpha-normalized hashes
  - Reuse the analysis results of dependency crates (item summaries, parts of Program) on the dependent side (today
    the sources are analyzed again)
  - Linking that replaces only the `.o` files that changed
  - Embedding libwild and keeping `.o` files in memory (no `.o` writes and no `exec`)
- **Incremental linking**
  - Leave padding in `.text` and replace only the functions that changed
  - Calls through a jump table (the blaze approach)
- **Cold builds**
  - Parallelize item summaries and parsing
  - Stop parsing the AST twice in cold builds
- **Size**
  - ICF in the compiler (hash of code + relocation targets)
  - Merge sections for strings
- **`-O`**
  - Our own inlining and stack-slot forwarding (to make up for Cranelift having no SROA)

## zust-analyzer: remaining differences from rust-analyzer

What is left after comparing with `bench/lsp_parity.py` and checking the references of std methods one by one
([docs/reports/2026-11-18-05-std-method-references.md](docs/reports/2026-11-18-05-std-method-references.md)).

### Type inference

- [ ] The type of `&*ptr` (dereferencing a raw pointer `*const T` → T, and the `&` in front of it).
  Example: `tf.iter()` after `let tf = unsafe { &*tf };` in `crates/sema/src/ast_infer.rs`
- [ ] Types of associated consts (`usize::BITS` → u32). Example: `(usize::BITS).to_string()`
- [ ] Closure parameters of std function arguments whose type comes from the error type and similar
  (`e` in `std::fs::write(..).map_err(|e| e.to_string())` is `io::Error`)
- [ ] Answer the type of a pattern binding at the binding itself as well (`zustc/typeAt` only answers at uses)
- [ ] `x` / `y` in a tuple pattern of a match arm, `(TokenTree::Literal(x), TokenTree::Literal(y))`
- [ ] Chains of method return types on types from dependency crates, such as `p.as_object_mut().unwrap()` (serde_json's
  `Value`)
- [ ] Values of numeric types, `k.to_string()` (when the `for` variable comes from a range `0..n` and the element type
  is not decided)
- [ ] The definition target of `next` on str's `Split<'_, P>` (mixed up with the impl for slice `Split`; str's
  iterator impls are inside core's `generate_pattern_iterators!` macro)

### Definitions and references

- [ ] rust-analyzer also counts `"x".to_string()` at the specialized impl (`impl ToString for str`, another line in
  string.rs). zustc only counts the trait declaration
- [ ] Only zustc counts `s.starts_with(k)` (k: &&str) in `ast_summary.rs` as `str::starts_with` (rust-analyzer resolves
  it differently)
- [ ] The one place where the reference count agrees only 97% on the zustc target (rust-analyzer: 100%)
- [ ] fixture: the rest of definition 465/467 and references 609/629

### Rename

- [ ] Renaming `Default::default` (rust-analyzer also rewrites inside dependency crates; we only rewrite the workspace.
  An intended difference)
- [ ] Renaming the alias in `use a::Point as Pt` (an intended difference)

### Speed and memory

- [ ] Slow cases at the references p90 (65 ms on the zustc target): resolving methods with common names across all files
- [ ] The dependency-crate index raised memory from 55 MB to 66 MB. Drop unused definitions / load them lazily
