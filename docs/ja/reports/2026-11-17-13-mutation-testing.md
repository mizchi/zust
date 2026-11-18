# ミューテーションテスト (cargo-mutants、2026-11-17)

日付: 2026-11-17

`cargo install cargo-mutants --locked` (27.1.0) を入れ、`bench/mutants.sh` で回す。

- 設定と範囲:
  - 設定は `.cargo/mutants.toml`。ビルドは `profile.mutants` (release と同じ最適化に増分ビルド)。
  - 変異を入れないもの: CLI の引数の処理・build.rs・テストとベンチのコード・Debug / Display。
  - 変異は全部で 18,240 個。1 個にテストで数秒〜1 分かかるので、全部は回さない。
- 範囲の絞り方:
  - `bench/mutants.sh diff [BASE]`: BASE (既定 origin/main) からの差分の行の変異だけ。変更の確認に。
  - `bench/mutants.sh pkg zust-ide`: クレートごと。
  - `bench/mutants.sh file crates/ide/src/symbols.rs -- -F match_score -j 2`: ファイル・関数の名前・並列の数を指定する。
  - `bench/mutants.sh list ...`: 数を見るだけ。
  - `--` の後ろは cargo mutants にそのまま渡す (`--shard 1/8` など)。
- テストに使うパッケージ:
  - crates/ide・crates/analyzer だけの変異: zust-ide と zust-analyzer のテスト。1 個 2〜3 秒。
  - それ以外: ワークスペース全部のテスト (コンパイラの層のテストの多くはルートの tests/ にある)。1 個 1 分ほど。
- 時間切れは、基準のテストの時間の 3 倍か 180 秒の大きい方。
- 結果は `mutants.out/` (missed.txt がテストで見つからなかった変異)。

試した結果:

| 対象 | 変異 | 見つかった | 見つからない | 時間 (-j 2) |
|---|---:|---:|---:|---:|
| `match_score` (ide) | 25 | 16 → 24 | 9 → 1 | 87 秒 |
| `KeyState::done` (base) | 7 | 5 | 2 | 5 分 |

- `match_score` の見つからなかった 9 個は語の頭の判定 (`_` 以外の区切り) で、テストが `_` しか見ていなかった。
  camelCase・`::`・空白・大文字が続く名前の場合を足した。
- 残りの 1 個 (`i > 0` → `i >= 0`) は等価な変異: i = 0 は前方一致で先に返るので、そこに来ない。
- `KeyState::done` の 2 個は、ハッシュの混ぜ方 (`^` → `|`、`<<` → `>>`)。キャッシュのキーは同じ入力で同じ値になれば
  動くので、機能のテストでは見えない (衝突しにくさの劣化)。
