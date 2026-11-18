# ide の見つからなかった変異を潰す (2026-11-17)

日付: 2026-11-17

ide (crates/ide) 全体の 3,142 個を `-j 3` で回し、2 時間で打ち切った (1,897 個まで)。結果は見つかった 1,165・
見つからない 668・時間切れ 44・ビルドできない 20。見つからない 668 個は全部 navigate.rs で、多いのは
ローカルの束縛 (`live` 79・`arm_binding` 48・`local_type` 48・`pat_binding` 20・`fn_param` 19)、`def_at` 60、
use の木 (`use_tree` 27・`resolve_use` 18・`reexport` 18)、文字列の小さな部品 (`targs`・`generic_names`・`captures`・
`cfg_pred`・`macro_bodies` など)。ゴールデン (fixture) はよくある書き方しか通らず、境界や分岐の片側を見ていなかった。

- 足したテスト: `crates/ide/src/navigate_tests.rs` (`#[path]` で navigate の子のモジュールにして、非公開の部品を直接呼ぶ)。
  - 文字列の部品: `captures` / `generic_names` / `targs` / `subst` / `alias_parts` / `ret_type` / `elem_type` /
    `index_type` / `type_head` / `name_hash` (FNV-1a の既知の値) / `mod_score` / `use_module` / `lib_rank`。
  - cfg: `any` / `all` / `not` / `target_os` / `target_family` / `unix` / `windows` / `miri` / 知らない述語。
  - `macro_rules!` の本体の範囲 (`{}` / `()` / 関数の中 / 閉じていないもの)、use の木 (入れ子・`as`・glob・`self`)。
  - ローカル: let / for / if let / while let / match の腕・ガード / or パターン / クロージャ / 引数のパターン /
    ブロックの中の束縛の寿命。束縛の判定 (`pat_binding` / `fn_param` / `arm_binding`) を正と負の両方で。
  - 型: 注釈 (`&'static P`・`&mut a::b::P`)・`T(..)`・`T { .. }`・`U::new()`・`a::b::P`。
  - derive のメソッド 16 個 (`clone` / `default` / `eq` / `ne` / `fmt` / `partial_cmp` / `lt` .. / `hash`)。
  - 書式の取り込み `{x}` / `{y:?}` / `{{x}}` の定義と参照、型引数 (fn / struct / impl / trait / enum / type / union)。
- テストで見つかった不具合 (直した):
  - match のガード `S { f: g } if g > 0 =>` の g が解決されなかった (束縛を `=>` で入れていた)。`if` がガードかを
    先読みし (`guard_arrow`)、ガードから見えるようにした。
  - `let v = a::b::P;` の v の型が `b` になっていた (モジュールの名前を型にしていた)。`a::b::T(..)` / `m::f()` も同じ。
  - 型注釈 `&'static P` の型が `static` になっていた (字句では寿命は `'` の後の識別子)。
  - or パターン `(m, 0) | (0, m)` の m が後ろの方に行っていた。rust-analyzer と同じく前の方に。
  - `apply_cfg` で、cfg で外れたアイテムの derive を数え直すときに、外れた範囲をまだ入れていなかった。
  - LSP がファイルの直下の `#[path = "x.rs"] mod m;` を `<ファイル名>/x.rs` で探していた (rustc はファイルのある
    ディレクトリから)。これで navigate_tests.rs が読まれず、zustc の参照の一致が 78% → 58% に下がって見えていた。
- 速さ: `python3 bench/lsp_parity.py` (zustc を対象) は defi 1.0 / hove 0.9 / comp 1.2 / refe 15.1 ms で、前と同じ
  (rust-analyzer は 0.9 / 0.9 / 5.6 / 27.3 ms)。一致も前と同じ (definition 100%・hover 82%・references 78%)。
  残りの references の違いは、rust-analyzer が数える cfg で外れたアイテムの中・OUT_DIR の生成ファイル・build.rs と
  共有する lexer.rs の中のもの。

テストを足した後、上の部品の関数 (37 個) の変異 967 個に絞って回し直した (`-F` で関数の名前、`-j 3` で 82 分。
見つからなかったものは、テストをさらに足して `--iterate` で回し直した):

| | 見つかった | 見つからない | 時間切れ |
|---|---:|---:|---:|
| 足す前 (全体の途中までの回の、同じ関数の分) | — | 441 | — |
| 1 回目 (967 個) | 715 | 213 | 39 |
| 2 回目 (残りの 249 個を `--iterate`) | 749 | 179 | 39 |

- 時間切れは `+=` → `-=` などで終わらなくなったもの (テストが止まるので見つかったのと同じ)。
- 残りの 179 個のほとんどは結果の変わらない変異:
  - 境界の `<` → `<=` (`sym` / `ident` が範囲の外を偽にするので同じ)。
  - 早い打ち切り (`arm_binding` / `pat_binding` の最初の除外、`guard_arrow` の `{` / `;` での打ち切り): 後ろの走査や
    `arm_pat` の範囲の確認でも同じ答えになる。速さのためのもの。
  - `generic_names` の `>` を探す走査: 結果は `targs` がもう一度区切るので同じ。ファイルの残り全部を写さないためのもの。
