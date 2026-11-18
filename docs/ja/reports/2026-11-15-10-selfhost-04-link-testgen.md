# セルフホストの第 4 段階: link.rs + testgen.rs

日付: 2026-11-15

- `std::path`：`Path`（= str）/ `PathBuf` / `OsStr` / `OsString`（= String）。`join` / `push` / `file_name` / `file_stem` / `extension` / `parent` /
  `with_extension` / `set_extension` / `display` / `to_string_lossy` / `is_file` / `is_dir` / `exists`（stat）
- `std::process::Command`：`new` / `arg` / `args` / `output` / `status`（fork + execvp。exec の失敗は close-on-exec の pipe で受け取り `Err` にする。
  stdout と stderr は poll で両方読む）、`ExitStatus`（`success` / `code` / Display）
- `env::var_os` / `env::split_paths`、`fs::remove_file`
- 同じ型への複数の `impl From<A> for T` / `impl From<B> for T`（呼び出し側が最初の引数の型で選ぶ）
- Deref の先が str の型のメソッド（`path_buf.join(..)` → str の trait のメソッド）、`os_string.push("..")`
- 書式文字列に直接書いた名前（`format!("{cands:?}")`）をクロージャのキャプチャとして数える
- 修正：`&&str` を受け手にした str の trait メソッドが 1 段余計に読んでいた
