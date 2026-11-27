# Trait objects keep their auto traits

Date: 2026-11-27

zustc's types did not keep `+ Send` / `+ Sync` of a trait object (`ast_types::tysyn` dropped them), and every
`dyn Trait` was taken as Send and Sync. A `Box<dyn Fn()>` holding an `Rc` could move to another thread
(tests/security/send_dyn_thread.rs, a TODO since [report 2026-11-27-01](2026-11-27-01-native-threads.md)). This report
keeps them, as rustc does.

## What rustc does

- `dyn Tr + Send` is Send, `dyn Tr + Sync` is Sync. A plain `dyn Tr` is neither: it may hold any type of its trait.
  It has an auto trait its trait has as a supertrait (`trait Tr: Send`).
- A value becomes a `dyn Tr + Send` only if its type is Send (E0277 at the coercion), and the same for Sync.
- A coercion may drop auto traits (`Box<dyn Tr + Send>` → `Box<dyn Tr>`), never add them (E0308).
- The order and repetition of auto traits do not matter: `dyn Tr + Send + Send` is `dyn Tr + Send`.

## What changed

- `ast_types::tysyn` writes the object's text with its auto traits after the trait, `Send` then `Sync`:
  `Fn() -> i32 + Send + Sync`. A repeated one is written once, so `dyn Tr + Send + Send` is the same type.
  `types::dyn_autos` splits such a text into the trait and its two flags.
- What reads the trait from the text takes it without the auto traits: the vtable (`to_dyn`), method calls
  (`dyn_trait_args`, `dyn_methods`, inference's method lookup), `Fn` calls (`dyn_fn_sig`), the region check of a
  `dyn Fn` call (`rg_dyn_fn_args`, which parses `0 as &dyn ..`, where `+ Send` would be an addition), and
  `Box<dyn Error + Send + Sync>`, which is the prelude's `__DynError` as `Box<dyn Error>` is. The rest already split the
  text at `+` (`implements`, the trait solver's auto traits, `impl dyn Tr` lookup).
- The three judges of the auto traits (`Program::auto_trait`, the trait solver's `auto_rec`, acg's
  `auto_trait_fails`) take a `dyn` type as Send / Sync only by its written auto traits or its trait's supertraits.
- `to_dyn`, which makes the vtable where a value becomes an object, checks the value's type against the object's auto
  traits: E0277, naming the part that is not (`Rc<i32>`, not the closure that captures it).
- A coercion of `Box`, `&` or `*const` of an object to one with fewer auto traits is the same value
  (`types::dyn_drops_autos`); one to more is still a mismatched type (E0308).
- A `static`'s type must be Sync (E0277). That check now comes after the auto traits of the program's traits'
  supertraits are read, so a `&dyn Stage` of `trait Stage: Sync` is Sync (consts/issue-54582.rs).

## What was checked

- tests/security: send_dyn_thread.rs passes (no longer a TODO); new cases send_rc_dyn_send_coerce.rs (an `Rc` closure
  as a `Box<dyn Fn() + Send>`), sync_cell_dyn_sync_coerce.rs (a `Cell` closure as a `&(dyn Fn() + Sync)`) and
  send_dyn_add_auto.rs (`Box<dyn Fn()>` to `Box<dyn Fn() + Send>`, E0308). Each is rejected where rustc rejects it.
- Against rustc: a `Box<dyn Tr + Send>` (and a `Vec` of them) moves to a thread; a `Box<dyn Tr>` does not, nor meets a
  `T: Send` bound; `Mutex<Box<dyn Tr + Send>>` works; `Box<dyn Error + Send + Sync>` with `?`, `Box::new(e)` and
  `"msg".into()` behaves as before.
- The rustc UI tests: 0 regressions, 4 newly passing (coercion/retslot-cast.rs,
  mismatched_types/trait-bounds-cant-coerce.rs, traits/bound/sugar.rs, traits/issue-7013.rs).
- `cargo test --release --workspace`.

## What is left

- An impl for an object with auto traits (`impl M for dyn Obj + Send`) is found by the trait alone: a call of its
  method on a `dyn Obj` is rejected as a mismatched type (E0308), where rustc reports E0599.
- `Arc<dyn Tr>` / `Rc<dyn Tr>` made from `Arc::new(x)` are not supported (with or without auto traits).
- `Unpin`, `UnwindSafe` and `RefUnwindSafe` in an object are still dropped.
