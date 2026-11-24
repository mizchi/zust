# カバレッジの見積もり

rustc の tests/ui の 90% を zustc が通すまで、あとどれくらいあり、残りにどれくらいかかりそうか (2026-11-24 時点)。
数字は `bench/forecast.py` が出したもので、このリポジトリの履歴だけを読む。後で見積もり直すときは次を実行する。

```
python3 bench/forecast.py --pending <開いている PR それぞれのブランチ (積み重ねならその順)>
```

レポートには次の部分がある。

- 今と、各日の終わりの、README の tests/ui の表
- 開いている PR が足す分 (それぞれの `bench/rust_ui_ratchet.txt` を、main から分かれたところと比べる)
- 直近のペースでの、目標までの日数
- マージされた変更それぞれが増やした分
- `bench/effort_log.tsv` を埋めた場合: 1 テストあたりのトークンと、残りにかかるトークン

## 要約 (2026-11-24)

| | pass | 対象 11,737 件のうち |
|---|---:|---:|
| main (README、235dae8) | 8,170 | 69.6% |
| main と開いている PR (#106, #121, #124, #122, #110)、compat-next44 | 約 8,343 | 約 71% |
| 90% | 10,564 | |

90% に届くには、あと約 2,220 件が通る必要がある。マージされた作業は、直近 2 日で 1 日あたり約 280 件を増やした。
1 日の増加は +982 から約 +260 まで下がってきている。先に通るのは易しいテストだからである。

| 前提 | 90% までの日数 (全種類) |
|---|---:|
| 1 日に同じ件数ずつ増える (線形) | 約 8 |
| 1 日に残りの同じ割合ずつ減る (幾何) | 約 13 |

**見積もり: 今の作業で 2〜3 週間。** 種類ごとにそれぞれ 90% に届くのは、もっと長くかかる。

- **compile-fail** が残りのテストの大半: 1 日約 240 件で +1,790、7〜13 日。
- **run-pass** は直近 2 日で 43 件しか伸びておらず、+305 が必要。残りの一部は、
  [SUPPORT.md](../../SUPPORT.md#large-features-not-started) が未着手として挙げる大きな機能を要する: unwinding
  (約 22 件)、executor (約 25 件)、FFI の構造体の値渡し (約 27 件)、本物のスレッド。直近のペースで 13〜19 日。
- **check-pass** は +107 が必要で、約 1 週間。
- **build-pass** は +20 が必要で、ほとんど動いていない。

これらの数字の意味は次のとおり。

- 「対象」は、`bench/rust_ui.py` が zustc に扱わせられる tests/ui のテスト: nightly 機能、補助クレート、特別な
  フラグが要らないもの。その 90% は Rust の 90% ではない。
- compile-fail のテストは、注釈が指す行で zustc がエラーを報告すれば通る。メッセージは比べない。
- README は `bench/rust_ui_accidental.txt` の偶然の pass (main で compile-fail 56 件) を数える。ratchet は数えず、
  したがって「未マージ」の列も数えない。

## トークンの見積もり

セッションは自分のトークン使用量を読めないので、トークン数は外から持ってくる。マージされた PR について、
`bench/effort_log.tsv` に 1 行をタブ区切りで書く: PR の番号、かかったトークン数 (使用量のダッシュボードから)、
任意で時間。すると `bench/forecast.py` は、それらの PR について次を出す。

- 増えたテスト 1 件あたりのトークン (その PR が上げた ratchet、全種類)
- その率に、目標までまだ必要なテスト数を掛けたもの

後のテストはもう済んだものより難しいので、結果は下限として受け取ること。下の表「What each merged change gained」が
各 PR のテストでの成果を示すので、どの PR を測るかを選ぶのに使える。

## レポート (bench/forecast.py、2026-11-24)

(スクリプトが出力したままの表)

### Measured (README of origin/main, 2026-11-24 08:43 UTC)

| kind | applicable | pass | rate |
|---|---:|---:|---:|
| run-pass | 2403 | 1857 | 77.3% |
| check-pass | 1813 | 1524 | 84.1% |
| build-pass | 135 | 102 | 75.6% |
| compile-fail | 7386 | 4687 | 63.5% |
| all | 11737 | 8170 | 69.6% |

### History (all kinds, the last measurement of each day, UTC)

| day | pass | rate | gain |
|---|---:|---:|---:|
| 2026-11-18 | 5546 | 47.3% |  |
| 2026-11-19 | 6528 | 55.9% | +982 |
| 2026-11-20 | 6920 | 59.2% | +392 |
| 2026-11-21 | 7446 | 63.7% | +526 |
| 2026-11-22 | 7850 | 67.2% | +404 |
| 2026-11-23 | 8111 | 69.4% | +261 |
| 2026-11-24 | 8170 | 69.6% | +59 |

### Not merged yet (bench/rust_ui_ratchet.txt of each branch)

| branch | run-pass | check-pass | build-pass | compile-fail |
|---|---:|---:|---:|---:|
| origin/claude/compat-next42 | +0 | +0 | +0 | +29 |
| origin/claude/compat-next43 | +0 | +0 | +0 | +78 |
| origin/claude/compat-next44 | +0 | +0 | +0 | +47 |
| origin/claude/sleepy-dijkstra-8sankk | +0 | +0 | +0 | +9 |
| origin/claude/serde-core-std | +1 | +1 | +0 | +8 |
| origin/claude/popular-crates | +0 | +0 | +0 | +0 |

(each branch against the ratchet of the commit it left REF at, and of the pending branches it is stacked on; a branch that did not raise its ratchet shows +0)

### Forecast to 90% (pace of the README's last 2.0 days)

| kind | pass now | + not merged | needed | tests/day | days (linear) | days (geometric) |
|---|---:|---:|---:|---:|---:|---:|
| run-pass | 1857 | +1 | 305 | 23 | 13.3 | 19.2 |
| check-pass | 1524 | +1 | 107 | 15 | 6.9 | 8.5 |
| build-pass | 102 | +0 | 20 | 0 | 40.3 | 61.4 |
| compile-fail | 4687 | +171 | 1790 | 241 | 7.4 | 13.1 |
| all | 8170 | +173 | 2221 | 280 | 7.9 | 13.0 |

### What each merged change gained (first-parent commits of origin/main that raised bench/rust_ui_ratchet.txt)

| time (UTC) | change | run-pass | check-pass | build-pass | compile-fail | tokens | tokens / test |
|---|---|---:|---:|---:|---:|---:|---:|
| 2026-11-21 11:40 | Merge pull request #24 from mizchi/claude/compat-next20 |  |  |  | +35 |  |  |
| 2026-11-21 14:34 | Merge pull request #26 from mizchi/claude/compat-next21 | +1 |  |  | +55 |  |  |
| 2026-11-21 16:13 | Merge pull request #27 from mizchi/claude/compat-next22 | +4 | +4 |  | +7 |  |  |
| 2026-11-21 18:37 | Merge pull request #32 from mizchi/claude/compat-next23 |  |  |  | +18 |  |  |
| 2026-11-21 23:00 | Merge pull request #39 from mizchi/claude/sleepy-dijkstra-8sankk | +2 | +2 |  | +249 |  |  |
| 2026-11-22 00:04 | Merge pull request #40 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +16 |  |  |
| 2026-11-22 00:57 | Merge pull request #36 from mizchi/claude/compat-next24 |  | +5 | +1 | +9 |  |  |
| 2026-11-22 02:14 | Merge pull request #42 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +12 |  |  |
| 2026-11-22 05:19 | Merge pull request #43 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +47 |  |  |
| 2026-11-22 05:46 | Merge pull request #41 from mizchi/claude/compat-next25 | +7 | +11 | +2 | +28 |  |  |
| 2026-11-22 06:33 | Merge pull request #46 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +16 |  |  |
| 2026-11-22 08:20 | Merge pull request #47 from mizchi/claude/compat-next29 |  |  |  | +5 |  |  |
| 2026-11-22 09:03 | Merge pull request #48 from mizchi/claude/sleepy-dijkstra-8sankk | +2 |  |  | +21 |  |  |
| 2026-11-22 11:33 | Merge pull request #51 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +20 |  |  |
| 2026-11-22 11:50 | Merge pull request #50 from mizchi/claude/compat-next30 |  |  |  | +21 |  |  |
| 2026-11-22 14:29 | Merge pull request #53 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +30 |  |  |
| 2026-11-22 15:44 | Merge pull request #52 from mizchi/claude/compat-next31 | +2 | +2 | +1 | +40 |  |  |
| 2026-11-22 16:18 | Merge pull request #55 from mizchi/claude/compat-next32 | +1 | +3 |  | +31 |  |  |
| 2026-11-22 18:06 | Merge pull request #57 from mizchi/claude/compat-next33 | +2 | +4 |  |  |  |  |
| 2026-11-22 19:39 | Merge pull request #54 from mizchi/claude/sleepy-dijkstra-8sankk | +6 | +7 |  | +28 |  |  |
| 2026-11-22 21:30 | Merge pull request #65 from mizchi/claude/compat-next35 | +3 | +1 |  | +4 |  |  |
| 2026-11-22 21:48 | Merge pull request #64 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +12 |  |  |
| 2026-11-23 05:49 | Merge pull request #67 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +21 |  |  |
| 2026-11-23 06:14 | Merge pull request #66 from mizchi/claude/compat-next36 | +5 | +1 |  | +13 |  |  |
| 2026-11-23 08:01 | Merge pull request #68 from mizchi/claude/compat-next37 | +1 | +1 |  | +25 |  |  |
| 2026-11-23 08:56 | Merge pull request #70 from mizchi/claude/compat-next38 |  |  |  | +9 |  |  |
| 2026-11-23 11:04 | Merge pull request #71 from mizchi/claude/sleepy-dijkstra-8sankk | +1 |  |  | +20 |  |  |
| 2026-11-23 12:45 | Merge pull request #72 from mizchi/claude/compat-next39 |  |  |  | +14 |  |  |
| 2026-11-23 14:10 | Region check part 16: deref coercions, block values and guards that... |  |  |  | +11 |  |  |
| 2026-11-23 15:12 | memchr builds for wasm32 (#90) |  | +2 |  |  |  |  |
| 2026-11-23 17:43 | Lints in expressions, E0308 where rustc points, more E0658 gates, E... |  |  |  | +79 |  |  |
| 2026-11-23 19:22 | Region check part 17: raw pointers, DerefMut receivers, scrutinee a... | +7 | +1 |  | +12 |  |  |
| 2026-11-23 19:36 | E0107 at GAT uses and dyn types, E0449, E0434, E0191 at dyn types (... |  |  |  | +42 |  |  |
| 2026-11-24 01:44 | Region check part 18: let extension through constructors, closure c... | +1 |  |  | +5 |  |  |
| 2026-11-24 02:30 | W1 of #38: proc macros run through the toolchain's proc-macro serve... |  | +3 |  |  |  |  |
| 2026-11-24 03:36 | Region check part 19: closures' calls and requirements, fn items as... | +6 | +1 |  | +10 |  |  |
| 2026-11-24 06:02 | Merge pull request #108 from mizchi/claude/tender-sagan-iatvun | +4 | +1 |  |  |  |  |
| 2026-11-24 06:29 | Merge pull request #107 from mizchi/claude/serde-core | +7 | +4 |  | +9 |  |  |
| 2026-11-24 07:48 | Region check part 20: closures against the regions their bounds bin... |  |  |  | +5 |  |  |
| 2026-11-24 08:38 | Region check part 21: fn items coerced to fn pointers, deref coerci... |  |  |  | +3 |  |  |

Tokens: bench/effort_log.tsv has no tokens yet. Write a row `pr<TAB>tokens<TAB>hours` for a merged PR (from the usage dashboard) and this report gives tokens per test and the tokens the rest would take.
