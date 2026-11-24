# borrow check を Polonius Alpha に寄せる (#1)

日付: 2026-11-24

rustc の nightly は 2026-08-04 から Polonius Alpha で借用を検査していて、stable も 2026 年末までにそうなる予定（[調査](2026-11-23-07-rust-frontier-survey.md)、[#1](https://github.com/mizchi/zust/issues/1#issuecomment-7110120344)）。zustc の borrow check は、もともとローカルごとに origin 1 つで借用をたどっていた（Polonius の見方）。ただし、ところどころに NLL の位置によらない規則が残っていて、別のところには zustc 独自の誤ったエラーもあった。この作業では答えを Alpha に寄せる。どの場合も rustc 1.102.0-nightly（2026-11-22、Alpha）と rustc 1.97.0（NLL）で確かめた。

## 変えたこと

1. **返される reborrow を関数の終わりまで生かさない。**
   - region check が、返される値の作る制約を記録する（`RgSt::ret_edges`、`rg_ret_at` の中）。
   - `Rg::forever`（universal region より長く生きなければならない reborrow。borrow check はその借用を最後まで生かす）と `Rg::holders`（reborrow がその型のリージョンより長く生きなければならないローカル。そのローカルが借用を持つ）は、その制約を除いたすべての制約をたどる。
   - その制約が成り立つのは値が返される地点だけで、そのあとには何も実行されない。だから、返されることでしか universal region に届かない reborrow は、それを持つローカルが生きているところでだけ生きる。これは Alpha の位置に依存する制約にあたる（`let b = &mut *a; if c { b } else { a }`）。
   - `holders` も効いていた。ループから `self.b` の借用を返す `fn next<'a>(&'a mut self) -> &'a str` では、`'a` を型に持つ `self` が借用を持つことになり、そのあとの周回すべてで借用が生きていた。
2. **ローカルの一部への代入でも借用を kill する。** `*list = n` や `list.0 = n` は、その部分が持っていた参照を通した借用を kill する（`Bk::kill_loans_through`）。外へ逃げた借用も含む。これは projection に対する Polonius の `loan_killed_at` にあたる。これまではローカル全体への代入でしか kill していなかった。
3. **`v` が参照のとき、`for x in v.iter_mut()` は `*v` を借用する。** ループの借用がローカル `v` そのものに付いていた。そのため、そういうループ（`v: &mut [T]` が引数）から `return x` すると「ローカル変数への参照を返している」となっていた。main にもある誤ったエラー。
4. **数値へのキャストは何も持たない。** `v as *mut _ as usize` は `v` の借用を持たない。`assert_eq!(v as *mut _ as usize, l.as_ptr() as usize)` で誤った衝突が出ていた。

`ZUSTC_NO_POLONIUS=1` で、1 と 2 を NLL の規則に戻せる。3 と 4 は誤ったエラーの修正なので切り替えない。

## 結果

`tests/ui/nll/polonius`（補助クレートを使わない 34 ファイル。受理か拒否かだけを比べた）:

| | 前（main） | 後 |
|---|---:|---:|
| Alpha が受理するのに zustc が拒否する | 3 | **0** |
| Alpha が拒否するのに zustc が受理する | 2 | 3 |
| 未対応 | 2 | 2 |

Alpha が拒否して zustc が受理するのは次の 3 件:

- `iterating-updating-cursor-issue-63908` と `-108704`。datalog の Polonius も受理する。108704 が前に拒否されていたのは、3 の誤った借用のためだけだった。健全なプログラムだが、rustc は拒否する。
- `lending-iterator-sanity-checks`。生きている 2 つの `T::Item<'_>` の結果について、E0499 を見逃している。ジェネリック関数のプレースホルダでの検査は、境界の trait のメソッドを呼ぶ関数をコストのために除いている（`placeholder_wave`）ので、この本体はそもそも borrow check されない。

`tests/ui/nll` と `tests/ui/borrowck` の全体で、`ZUSTC_NO_POLONIUS` の 2 つの規則によって判定が変わるのは 4 ファイル。4 件とも前は拒否し、いまは Alpha と同じく受理する: `nll/polonius/assignment-kills-loans`、`nll/polonius/iterating-updating-mutref`、`borrowck/issue-62007-assign-box`、`borrowck/issue-62007-assign-field`。

手で書いた例（`tests/regionck.rs` の `polonius_alpha`）: Alpha が受理して NLL が拒否するもの 5 件、両方が受理するもの 4 件、両方が拒否するもの 10 件。すべて Alpha と一致する。`ZUSTC_NO_POLONIUS=1` にするとこのテストは失敗する。

`bench/rust_ui.py --check` をベースラインと比べた: 回帰 0 件。新しく通るものがないのは、これらのテストが `//@ revisions` を持っていて、スクリプトが飛ばすから。

`zustc check -j 1` の命令数（cachegrind）、前（0e87d40）と後:

| | 前 | 後 | 変化 |
|---|---:|---:|---:|
| gen5000 | 3780.31 M | 3782.31 M | +0.05% |
| patterns（`mix 8`） | 1342.47 M | 1342.60 M | +0.01% |
| traits（`gen_traits.py 300`） | 10043.98 M | 10049.56 M | +0.06% |

`cargo test --release --workspace` は、main でも失敗する `crates/analyzer/tests/lsp.rs` の 2 件を除いて通る。

## 残っていること

- 境界の trait のメソッドを呼ぶ本体はプレースホルダで検査しないので、lending iterator のようなものは borrow check されない。
- zustc は、付け替える `&mut` を通してループをまたぐ流れを datalog の Polonius と同じように扱う（63908、108704）。Alpha はこれを拒否する。合わせるには、付け替えたローカルのリージョンが kill の前に必要とされていた場所で、kill した借用を生かしておく必要がある。
- `bench/rust_ui.py` は revision を選ばないので、revision を持つ `nll/polonius` と `borrowck` のテストはどこにも数えられていない。ここでの比較は、リポジトリの外の小さなスクリプトでした。
