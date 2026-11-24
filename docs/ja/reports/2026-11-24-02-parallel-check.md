# `check` を fork した子プロセスで分ける (#101)

日付: 2026-11-24

`zustc check` は 1 つのプロセスで動いていた。ビルドは項目の要約も検査のループも fork して分けていたのに、check はどちらも分けていなかった。4 コアでは、gen5000 の cold な `check` は、コードを生成しないのに `-j 4` の cold なビルドと同じくらいかかっていた。この作業では、ビルドと同じように、check の各 wave を fork した子プロセスに分ける（[#101](https://github.com/mizchi/zust/issues/101)）。

## 変えたこと

- `zustc check` が `-j N` を受け付け、既定は CPU の数にした。これまでは常にジョブ 1 つだったので、項目の要約さえ分けていなかった。
- 検査のループ（`Session2::check`、`crates/core/src/driver2.rs`）は、wave を 3 段で処理するようにした。
  1. 順に、記録とキャッシュを引く。外れたものを集める。
  2. `check_many` が外れたものを検査する。このプロセスで検査するか、ジョブ 1 つあたり 32 個以上（`ZUSTC_CHECK_FORK_MIN`）あれば fork した子で検査する。子 k は `i % jobs == k` のものを検査し、それぞれの Blob（キャッシュと同じく名前で直列化したもの）かエラーを返す。
  3. wave の順に結果を受け取る。キャッシュに書き、エラーを残し、記録（check の memo）を作る。

  子の Blob を読み戻すと、その関数が要求したインスタンスがもう一度要求される。だから次の wave は 1 プロセスのときと同じになり、診断は `-j` によらない。
- zust-analyzer はこれまでどおりジョブ 1 つで検査する（セッションは `jobs: 1`）。編集のたびの 1 関数の検査は、どのみちしきい値に届かない。

## 子が結果のほかに送らなければならないもの

`-j 1` と `ZUSTC_CHECK_FORK_MIN=1 -j 4`（ほぼすべての wave で fork する）を tests/ui で比べて、最初の版が失っていたものが 3 つ見つかった。

1. **lowering が作るのに返さないインスタンス。** lowering が要求するインスタンスの一部は `pending` に積まれるが、返す `req` には入らない。1 プロセスなら次の wave で検査されるが、子からでは親に伝わらなかった。子は、その関数の検査中に作ったインスタンス（`pending` の増えた分を順に）を送り、そのあとに `req` の残りを送るようにした。
2. **失敗した関数が残したもの。** 検査に失敗した関数も、インスタンス（クロージャの本体）を要求し、クロージャに自分のリージョンについて伝えている（`clo_facts`）。子はメッセージしか送っていなかった。いまはそのインスタンスと事実を Blob にしてエラーと一緒に送り、親がそれを読み戻す。
3. **キャッシュの直列化が受け付けない型引数を持つインスタンス。** 子が `types_enc` で panic していた（`pattern/slice-patterns-irrefutable.rs`）。そういう関数は「送れない」と印を付け、親が自分で検査する。

## 測った結果

4 コア、main d2146f7 とこのブランチ。

実時間（5 回の中央値）:

| | main `-j 1` | main `-j 4` | #101 `-j 1` | #101 `-j 4` |
|---|---:|---:|---:|---:|
| `check` gen5000 | 529 ms | 516 ms | 536 ms | **250 ms** |
| `check` patterns（`gen_patterns.py mix 8`） | 279 ms | 261 ms | 276 ms | **181 ms** |

この変更の前は、`-j` は無視されていたので `check` に効かなかった。

gen5000 を `-j 4` で `ZUSTC_PROF=1`: summaries 89 → 53 ms、check 298 → 122 ms。

命令数（cachegrind、KPI のスクリプトがいま渡す `-j 1`）:

| | main | #101 | 変化 |
|---|---:|---:|---:|
| gen5000 check | 3769.85 M | 3772.64 M | +0.07% |
| patterns check | 1335.27 M | 1336.54 M | +0.09% |

増えた分は wave の 3 段（外れたものと記録の項目をベクタに持つこと）。`-j 1` では fork しない。

gen5000 での `zustc` プロセスのピーク RSS: `-j 1` で 137 MiB、`-j 4` で 49 MiB。`-j 4` では関数の本体の構文木を親ではなく子が作る。子それぞれのピークは測っていない。

## どの `-j` でも出力が同じこと

- tests/ui の 6000 ファイル（固定のシード）で、`-j 1` と `ZUSTC_CHECK_FORK_MIN=1 -j 4` を比べた: 5996 件が同じで、3 件が時間切れ。違った 1 件は `layout/uninitialized-gat-projection-cycle-issue-153205.rs` で、main でもスタックが溢れるもの。fork すると子が溢れ、親がその関数を検査し直してまた溢れるので、メッセージが 2 回出る。
- 同じ 6000 ファイルで、このブランチの `-j 1` と main を比べた: main でも panic する 2 件（`codegen/normalization-overflow/*`）のバックトレースに新しい `check_many` のフレームが入る以外は同じ。panic のメッセージのスレッド番号は実行ごとに変わるので伏せて比べた。
- gen5000、`gen_traits.py 2000`、`gen_patterns.py mix 8`: `-j 1`、`-j 4`、`ZUSTC_CHECK_FORK_MIN=1` でバイト単位で同じ。
- `tests/check_parallel.rs`: 300 個の関数と、上の場合（クロージャを要求したあとに失敗する関数、クロージャへのリージョンの事実を残して失敗する lowering、クロージャの中のエラー、プレースホルダのインスタンス）。これを `jobs: 1` と `jobs: 4` で、キャッシュなしとありで検査する。(2) の修正を外すと失敗する。fork のときにほかのテストのスレッドがロックを握っていないよう、独立したバイナリの 1 つのテストにした。
- `cargo test --release --workspace`: `zust-analyzer` の `tests/lsp.rs`（`hover_and_completion`、`type_inference_like_rust_analyzer`）を除いてすべて通る。この 2 件はこの環境では main でも失敗する。

## KPI のスクリプト

valgrind は fork した子を数えないので、命令数を数えるスクリプトは `check -j 1` を実行するようにした: `bench/kpi_track.py`、`bench/ci_bench.py`、`bench/patterns.py`。`bench/rust_ui.py` も、多くの check を同時に走らせるので `-j 1` を渡す。実時間の `check` の行（`ci_bench.py`、したがって README の数字）は並列の check を測るようになるので、CI のランナーでは下がる。

## 残っていること

- セルフホストは確かめられなかった: `tests/selfhost/build_compiler.py` のソースは main の時点ですでに止まる（``file not found for module `dead` ``。TODO.md にあるとおり rustc でもビルドできない）。
- 検査の前の小さな段（`prep`、`split`、prelude、`program`: gen5000 で約 40 ms）は、まだ 1 プロセスで動く。
- 子がある関数で落ちると、親がその子の受け持ちの残りを自分で検査するので、落ちたことが 2 回表示される。
