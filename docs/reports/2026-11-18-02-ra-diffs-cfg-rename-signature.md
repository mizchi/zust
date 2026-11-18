# rust-analyzer との残りの違いを詰める: cfg・trait の impl の rename・signatureHelp (2026-11-18)

日付: 2026-11-18

- **cfg で外れたアイテムの中**: rust-analyzer に小さいワークスペースで聞いて確かめた振る舞いに合わせた。
  - rust-analyzer は外れたアイテム (関数・impl・struct・enum・const・type・trait・インラインの mod・文の cfg) の中も、
    外側のスコープから見えるアイテムへのパス (`used()` / `P` / `sub::deep` / `crate::P` / use で取り込んだ名前) は解決して、
    definition / references / hover に答える。ローカル・フィールド・メソッド (`p.get()` / `s.x`)・関連アイテム (`P::new`)・
    外れたアイテムの名前そのものには答えない。前の zustc は外れたものを全部除いていた。
  - `def_at` が外れた所のトークンを `inactive_def_at` で引き、上の範囲だけを答える。references・rename・hover もこれを通る。
  - `apply_cfg`: 設定されていない cfg を「成り立つ」としていたのを、rustc と同じく「成り立たない」にした
    (`no_global_oom_handling` / `miri` / `doc` / 知らない `key = ".."`)。`target_arch` / `target_pointer_width` /
    `target_endian` / `target_env` / `target_vendor` / `target_has_atomic` / `panic` を分かるようにした。`test` と
    `debug_assertions` は rust-analyzer と同じく成り立つとする。
- **trait の impl のメソッドの rename** (`impl Default for S` の `fn default`): rust-analyzer は std の宣言と依存のクレートの
  ソースまで 1,273 か所書き換える (そのままでは壊れる書き換え)。zustc はここは合わせず、
  - trait がワークスペースのもの: trait の宣言から引き直して、宣言・全部の impl・呼び出しを書き換える。
  - ライブラリ・外部のクレートの trait: 理由を付けて断る (宣言の方を変えられないので)。
- **signatureHelp**:
  - `st.extend(..)` (`Vec<String>`) に答えなかったのは、std の `impl Extend<T> for Vec` と `impl Extend<&T> for Vec` の
    2 つの候補でどちらも選ばなかったため。同じファイルの同じ持ち主・同じ名前のメソッドなら先のもの (総称的なもの) にした。
    definition / hover も同じく答えるようになった。
  - クロージャの呼び出し `h(..)` (`let h = |name: &str| -> Option<Key> { .. }`): `impl Fn(name: &str) -> Option<Key>` を
    シグネチャにする (戻り値を書いていなければ本体の式の型)。rust-analyzer は `Key` を展開して `u128` にする。
  - rust-analyzer と同じく doc コメントを `documentation` に付けた。
- **式の型のリテラル**: `"a"` → &str、`'c'` → char、`b"x"` → &[u8]、`b'x'` → u8、`1u8` / `2.0f32` / `1_000usize`、
  `true` → bool。式全体がリテラルのときだけ (`x + 1` の 1 は式の型ではない)。接尾辞のない数 (`0` / `1.5`) は、
  使い方で型が決まる (rust-analyzer は `let mut acc = 0; acc = add(acc, x_i64)` の acc を i64 にする) ので型にしない。

結果 (`python3 bench/lsp_parity.py`、zustc を対象。synth は全部 100% のまま):

| 機能 | 前 | 後 | rust-analyzer |
|---|---:|---:|---:|
| references (数の一致) | 78% | 92% | 100% |
| documentHighlight | 85% | 100% | 100% |
| rename (書き換えの一致) | 82% | 94% | 100% |
| signatureHelp | 89% (答えた 85%) | 100% (答えた 95%) | 100% (答えた 95%) |
| 時間の中央値 refe / rena / sign (ms) | 14.5 / 1.3 / 1.3 | 14.2 / 1.3 / 1.2 | 25.0 / 0.8 / 0.5 |

fixture (`python3 bench/ide_golden_ra.py`): definition 460 → 462/467、references 584 → 606/629 (93% → 96%)、
hover 612 → 614/636。

残りの違い: rename の `default` (上の、わざと断るもの)。references の `u8` / `Key` (rust-analyzer は OUT_DIR の生成
ファイルと、build.rs と共有する lexer.rs の中も数える)。hover の `ZUSTC_INLINE_LOG` (文字列の中)。
