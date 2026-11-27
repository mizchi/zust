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
  `types::dyn_autos` splits such a text into the trait and its two flags. An object of auto traits alone
  (`Box<dyn Send>`, `&dyn Sync`) has the first as its trait: `dyn Sync + Send` is `Send + Sync`, both kept.
- What reads the trait from the text takes it without the auto traits: the vtable (`to_dyn`), method calls
  (`dyn_trait_args`, `dyn_methods`, inference's method lookup), `Fn` calls (`dyn_fn_sig`), the region check of a
  `dyn Fn` call (`rg_dyn_fn_args`, which parses `0 as &dyn ..`, where `+ Send` would be an addition), and
  `Box<dyn Error + Send + Sync>`, which is the prelude's `__DynError` as `Box<dyn Error>` is. The rest already split the
  text at `+` (`implements`, the trait solver's auto traits, `impl dyn Tr` lookup).
- The three judges of the auto traits (`Program::auto_trait`, the trait solver's `auto_rec`, acg's
  `auto_trait_fails`) take a `dyn` type as Send / Sync only by its written auto traits or its trait's supertraits.
- `to_dyn`, which makes the vtable where a value becomes an object, checks the value's type against the object's auto
  traits, also an auto trait that is the object's trait (`dyn Send`): E0277, naming the part that is not, through
  closures' captures and the program's own structs (`Rc<i32>`, not the closure that captures it, nor the pointer
  inside the `Rc`). In a constant's initializer the error is at the constant's line (`zustc check` too).
- `TraitDb::gives_auto` looks through all of a trait's supertraits before it calls the answer open: with
  `trait Tr: A + Error` and `trait A: Send`, `dyn Tr` is Send though zustc does not know `Error`'s supertraits.
- The type hash of a `dyn Tr + Send` takes in `Tr`'s methods, as a `dyn Tr`'s does.
- A coercion of `Box`, `&` or `*const` of an object to one with fewer auto traits is the same value; one to more is
  still a mismatched type (E0308). `dyn_coerces` decides it: an auto trait the object's trait has as a supertrait counts
  as one it has (a `Box<dyn Job>` of `trait Job: Send` is a `Box<dyn Job + Send>`), and an object of auto traits alone
  is the set of them (`Box<dyn Send + Sync>` is a `Box<dyn Sync>`).
- A pointer cast may drop an object's auto traits, not add them: `*const dyn Job as *const (dyn Job + Send)` is E0804,
  as rustc's (cast/ptr-to-trait-obj-add-auto.rs passes). As rustc's, this is for an object with a principal trait: one
  of auto traits alone has no methods, and `*const dyn Sync as *const (dyn Send + Sync)` is accepted
  (tests/security/send_dyn_ptr_cast_auto_only.rs; before, `dyn Send` to `dyn Send + Sync` was refused).
- `&(dyn Tr + Send)` → `*const dyn Tr` (a reference coerced to a raw pointer) may drop auto traits too, by the same
  `dyn_coerces`, as a coercion and as an `as` cast (tests/security/send_dyn_ref_to_ptr_drop_auto.rs).
- The program's own structs with fields are judged by their fields, not their type arguments (a `Maker<Rc<u32>>` of
  a `fn() -> T` field is Send).
- A `static`'s type must be Sync (E0277). That check now comes after the auto traits of the program's traits'
  supertraits are read, so a `&dyn Stage` of `trait Stage: Sync` is Sync (consts/issue-54582.rs).

## What was checked

- tests/security: send_dyn_thread.rs passes (no longer a TODO); new cases send_rc_dyn_send_coerce.rs (an `Rc` closure
  as a `Box<dyn Fn() + Send>`), sync_cell_dyn_sync_coerce.rs (a `Cell` closure as a `&(dyn Fn() + Sync)`),
  send_rc_bare_dyn_send.rs (an `Rc` as a `Box<dyn Send>`), send_dyn_add_auto.rs (`Box<dyn Fn()>` to
  `Box<dyn Fn() + Send>`, E0308) and send_dyn_ptr_cast_add_auto.rs (E0804). Each is rejected where rustc rejects it.
- Against rustc: a `Box<dyn Tr + Send>` (and a `Vec` of them) moves to a thread; a `Box<dyn Tr>` does not, nor meets a
  `T: Send` bound; `Mutex<Box<dyn Tr + Send>>` works; `Box<dyn Error + Send + Sync>` with `?`, `Box::new(e)` and
  `"msg".into()` behaves as before.
- The rustc UI tests: 0 regressions, 5 newly passing (coercion/retslot-cast.rs,
  mismatched_types/trait-bounds-cant-coerce.rs, traits/bound/sugar.rs, traits/issue-7013.rs,
  cast/ptr-to-trait-obj-add-auto.rs).
- `cargo test --release --workspace`.

## What is left

- An impl for an object with auto traits (`impl M for dyn Obj + Send`) is found by the trait alone: a call of its
  method on a `dyn Obj` is rejected as a mismatched type (E0308), where rustc reports E0599.
- `Arc<dyn Tr>` / `Rc<dyn Tr>` made from `Arc::new(x)` are not supported (with or without auto traits).
- `Unpin`, `UnwindSafe` and `RefUnwindSafe` in an object are still dropped.
