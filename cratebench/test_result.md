# cratebench: `zurgo test` と `cargo test`

`python3 cratebench/test.py --write` の結果 (lib・bin・tests/ のテスト。doc テストは数えない)。cargo の列は cargo での結果の数。

| クレート | 結果 | cargo: ok / FAILED / ignored | 詳細 |
|---|---|---:|---|
| adler2 | match | 6 / 0 / 0 | テスト 6 個 |
| cfg-if | match | 3 / 0 / 0 | テスト 3 個 |
| either | zurgo-fails | 8 / 0 / 0 | error: crate `either`: src/lib.rs:1810:14: type annotations needed for an empty array |
| fnv | match | 1 / 0 / 0 | テスト 1 個 |
| hex | zurgo-fails | 16 / 0 / 0 | error: two packages give the library `bitflags`: `bitflags v1.3.2` and `bitflags v2.13.2`. Two versions of one crate are not supported yet (mizchi/zust#28) |
| itoa | zurgo-fails | 11 / 0 / 0 | error: two packages give the library `itoa`: `itoa v1.0.19` and `itoa v1.0.19`. Two versions of one crate are not supported yet (mizchi/zust#28) |
| rustc-hash | match | 7 / 0 / 0 | テスト 7 個 |
