# cratebench

crates.io の小さなクレートを zustc でビルドできるか・どのくらい速いかを rustc と比べる。

- `vendor/<名前>/`: クレートの上流のリポジトリ (git submodule)。crates.io の配布物と `src/` が同じコミットに固定してある。
  取ってくるには、リポジトリの根で `git submodule update --init cratebench/vendor`

  | クレート | 上流 | 固定した版 |
  |---|---|---|
  | fnv | servo/rust-fnv | v1.0.7 (lib.rs がリポジトリの直下。run.py が src/lib.rs に置き直す) |
  | adler2 | oyvindln/adler2 | 2.0 (タグがないので main のコミット) |
  | hex | KokaKiwi/rust-hex | v0.4.3 |
  | itoa | dtolnay/itoa | 1.0.19 |
  | rustc-hash | rust-lang/rustc-hash | v2.1.3 |
  | cfg-if | rust-lang/cfg-if | v1.0.4 |
  | either | rayon-rs/either | 1.19.0 |
- `apps/<名前>/main.rs`: そのクレートを使うプログラム (最後に `done` を出す。edit の計測はこの行を書き換える)
- `crates.txt`: 名前・edition・feature
- `run.py`: `python3 cratebench/run.py [名前..]` (experiment のディレクトリで、`cargo build --release` の後)

結果は [../docs/reports/2026-11-16-21-cratebench.md](../docs/reports/2026-11-16-21-cratebench.md)。
