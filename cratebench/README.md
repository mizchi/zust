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

## test.py: `zurgo test` と `cargo test`

`python3 cratebench/test.py [名前..] [--check] [--write]` (リポジトリの根で、`cargo build --release -p zurgo` の後)。
vendor/ のクレート自身のテスト (lib・bin・tests/。doc テストは数えない) を out/cratebench-test/ に写して、
`cargo test` と `zurgo test` で動かし、(ターゲット, テスト) ごとに ok / FAILED / ignored を比べる。

- `locks/<名前>.lock`: 両方が使う Cargo.lock (`--locked`)。ないものは cargo が作り、ここに残す
- `test_passing.txt`: cargo と一致したクレート。`--check` (CI) はこれらが一致し続けることを確かめる
- `test_result.md`: `--write` が書く、すべてのクレートの結果

## ui/: UI フレームワーク (dioxus / leptos) を wasm32 向けに

- `ui/apps/<名前>/`: そのフレームワークのカウンタのアプリ (Cargo.toml と Cargo.lock で版を固定。zust の Cargo の
  ワークスペースには入らない)
- `ui/run.py`: `python3 cratebench/ui/run.py [名前..] [--rustc]` (リポジトリの根で、`cargo build --release --workspace` の後。
  クレートを crates.io から取るのでネットワークが要る)。`zurgo metadata` の wasm32 向けのグラフを、葉から順に
  zustc でビルドし、失敗するクレートとその最初のエラーを、その上にあるクレートの数の順に出す。`--rustc` は先に
  `cargo build --target wasm32-unknown-unknown` で、rustc ならビルドできることを確かめる

結果は [../docs/reports/2026-11-21-03-ui-frameworks-wasm.md](../docs/reports/2026-11-21-03-ui-frameworks-wasm.md)。
