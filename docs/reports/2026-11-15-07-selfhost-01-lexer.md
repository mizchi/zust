# Toward std coverage (self-hosting stage 1: zustc's own lexer)

Date: 2026-11-15

I combined zustc's own `src/lexer.rs` into one file (`tests/selfhost/build_lexer.py`), compiled it with zustc, and added what was missing one by one.
- `std::hash`: `Hash` / `Hasher` (including the default methods `write_u8` to `write_usize`, `write_str`, `write_length_prefix`), `impl Hash` for integers, bool, char, str, String, slices and Vec,
  and `DefaultHasher` (the same SipHash-1-3 with key 0 as std, so the values also match std)
- Method calls on trait implementations for str and slices (`impl Hash for str` / `impl<T: Hash> Hash for [T]`) (looked up before the built-in methods)
- Type aliases `type Key = u128;` / `type Res<T> = Result<T, String>;`
- `std::fs::read_to_string` / `read` / `write`, `std::env::args` / `var`, `std::io::Error` (Display / Debug in the same form as std: `Os { code: 2, kind: NotFound, message: ".." }`), `std::io::Result<T>`
- `String::from_utf8_lossy` (U+FFFD per broken sequence, as in std), `bool::then` / `then_some`, integer `to_le_bytes` / `to_be_bytes` / `to_ne_bytes` / `from_le_bytes` etc., u8 `is_ascii_alphanumeric` etc.
- Byte string patterns (`match w { b"pub" | b"fn" => .. }`, w: &[u8]), `&[u8] == b"pub"`, `&&str` passed as a `&str` argument (deref coercion)
- `macro_rules!` defined inside a function
- A closure whose body uses a capture as a value (`changed.then(|| String::from_utf8(out))`) captures it by value, as rustc does (FnOnce). A move out of a by-value capture zeroes it on the environment side
- New prelude items (Hash / Hasher / Args etc.) are attached only when their names are needed (common names like `fn write` / `len` do not pull them in)
