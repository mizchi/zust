# Self-hosting stage 4: link.rs + testgen.rs

Date: 2026-11-15

- `std::path`: `Path` (= str) / `PathBuf` / `OsStr` / `OsString` (= String). `join` / `push` / `file_name` / `file_stem` / `extension` / `parent` /
  `with_extension` / `set_extension` / `display` / `to_string_lossy` / `is_file` / `is_dir` / `exists` (stat)
- `std::process::Command`: `new` / `arg` / `args` / `output` / `status` (fork + execvp. An exec failure is received through a close-on-exec pipe and becomes `Err`.
  stdout and stderr are both read with poll), `ExitStatus` (`success` / `code` / Display)
- `env::var_os` / `env::split_paths`, `fs::remove_file`
- Multiple `impl From<A> for T` / `impl From<B> for T` for the same type (the call site chooses by the type of the first argument)
- Methods of types that Deref to str (`path_buf.join(..)` → a method of a str trait), `os_string.push("..")`
- Names written directly in format strings (`format!("{cands:?}")`) are counted as closure captures
- Fix: str trait methods with a `&&str` receiver read one extra level
