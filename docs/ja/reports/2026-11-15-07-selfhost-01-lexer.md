# std の範囲へ（セルフホストの第 1 段階: zustc 自身の lexer）

日付: 2026-11-15

zustc 自身の `src/lexer.rs` を 1 ファイルにまとめて（`tests/selfhost/build_lexer.py`）zustc でコンパイルし、足りないものを順に足した。
- `std::hash`：`Hash` / `Hasher`（`write_u8` 〜 `write_usize`・`write_str`・`write_length_prefix` の既定のメソッドも）と、整数・bool・char・str・String・スライス・Vec の `impl Hash`、
  `DefaultHasher`（std と同じ SipHash-1-3・鍵 0 で、値も std と一致する）
- str とスライスへの trait 実装（`impl Hash for str` / `impl<T: Hash> Hash for [T]`）のメソッド呼び出し（組み込みのメソッドより先に探す）
- 型の別名 `type Key = u128;` / `type Res<T> = Result<T, String>;`
- `std::fs::read_to_string` / `read` / `write`、`std::env::args` / `var`、`std::io::Error`（Display / Debug は std と同じ形: `Os { code: 2, kind: NotFound, message: ".." }`）、`std::io::Result<T>`
- `String::from_utf8_lossy`（std と同じく壊れた列ごとに U+FFFD）、`bool::then` / `then_some`、整数の `to_le_bytes` / `to_be_bytes` / `to_ne_bytes` / `from_le_bytes` など、u8 の `is_ascii_alphanumeric` など
- バイト列のパターン（`match w { b"pub" | b"fn" => .. }`、w: &[u8]）、`&[u8] == b"pub"`、`&&str` を `&str` の引数に（deref の強制変換）
- 関数の中で定義した `macro_rules!`
- 本体がキャプチャを値として使うクロージャ（`changed.then(|| String::from_utf8(out))`）は、rustc と同じく値でキャプチャする（FnOnce）。値で持ったキャプチャからのムーブは、環境の側を 0 にする
- prelude の新しい項目（Hash / Hasher / Args など）は、その名前が要るときだけ付ける（`fn write` / `len` などのよくある名前では引かない）
