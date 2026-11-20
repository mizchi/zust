# `std::thread` をグリーンスレッドに

Date: 2026-11-20

zustc の `std::thread::spawn` はクロージャをその場で実行し、`join` がその値を返していた（[報告 2026-11-19-01](2026-11-19-01-formal-models.md) の欠陥 5）。これは Rust が許す実行順の 1 つだが、他のスレッドが後で行うことを待たないスレッドの場合に限られる。2 つのプログラムがおかしくなっていた（`formal/repro/thread_spawn_*.rs`）。

- `thread_spawn_channel.rs`: 子が `recv` し、main が spawn の後で send する。rustc は `child got 42` を出すが、zustc では子が空のチャネルで recv して panic していた。
- `thread_spawn_mutex.rs`: main がロックを持ったまま spawn し、子もロックする。出力は正しかったが、main がロックを持っている間に子がロックを取っていた（`Mutex::lock` がブロックしなかった）。

main が spawn の後で立てる atomic の旗を待つ空回り（`while !FLAG.load(..) {}`）は終わらなかった。

## 選んだもの

3 つの方法を比べた。

- 本物の OS スレッド（pthread）。`Arc` と atomic を本物の atomic にする必要があり、そのためには 2 つのバックエンド（Cranelift と minclif）の両方に atomic 命令が要る。`Mutex` とチャネルには futex が要り、prelude の `static mut` はスレッドごとにし、子スレッドの panic は `join` の `Err` にしなければならない。いちばん大きな変更で、コード生成にまで及ぶ。
- クロージャの実行を遅らせ、`join` のときか、スレッドが待つことになったときに走らせる。小さく済むが、交互に進むスレッド（2 本のチャネルでの ping-pong）は扱えない。
- グリーンスレッド。スレッドごとにスタックを持ち、スレッドが待つところで切り替える。これを選んだ。交互に進むスレッドも扱え、OS スレッドは 1 本のままなので、`Arc`（`Rc` と同じ）と atomic（ただのセル）も健全なままである。2 つの切り替え点の間で他のものが走ることはない。

## 設計

実行環境は prelude にある（`crates/syntax/src/prelude_lib.rs.in` の `std::thread`）。

- スケジューラは、スレッドの一覧（スレッド 0 が main）、いま走っているスレッド、世代番号を持つ。世代番号は、他のスレッドが待ちうることが起きるたびに増える（send、unlock、スレッドの終わり）。
- `spawn` は 1 MiB のスタックと `ucontext_t` を確保し、libc の `getcontext` / `makecontext` で入口を設定する。そしてすぐ新しいスレッドに切り替える（`swapcontext`）。だから待たないスレッドは前と同じように走り、そういうプログラムの出力の順序も変わらない。
- スレッドが待つのは、`join`、空のチャネルでの `recv`（とチャネルの `iter`）、他のスレッドが持つ `Mutex`、`sleep`、`yield_now`。atomic の `load` も、他のスレッドが生きている間は譲るので、空回りのループでも他のスレッドが走る。待つときは、終わっていない次のスレッドを順番に走らせる。
- デッドロックは、他のスレッドが全員待っていて、一巡しても何も変わらない場合である。そのとき `join` は "deadlock" で panic し、`recv` は `Err` を返し（送り手が全員いなくなったかのように。スレッドが 1 本のときの前の動きと同じ）、`lock` はそのまま進む。
- `MutexGuard` は `Drop` でロックを解放するようにした。Rust が drop するところで guard が drop されない場合でも、後の `lock` は他のスレッドを待った後にそのまま進むので、スレッドが 1 本のプログラムは前と同じように動く。

prelude の修正が 1 つ要った。prelude は、プログラムが使うアイテムだけに名前で絞られるが、`static mut X` の名前が `mut` と取られていた。ほとんどのプログラムは `mut` と書くので、prelude の `static mut` はすべて、どのプログラムにも入っていた。そのためスケジューラの static がスレッドの構造体を `hello` にまで引き込み、自分で `Box` を定義するプログラムとぶつかった。今は `static mut X` の名前は `X` である（`crates/syntax/src/prelude.rs` の `defined_name`）。

## 確かめたこと

- `tests/green_threads.rs` は `tests/green_threads/` のプログラムをビルドし、その出力を rustc のものと比べる。対象は次のとおり。
  - 2 つの再現プログラム
  - 2 本のチャネルでの ping-pong
  - 4 つの生産者と 1 つの消費者
  - 子がロックしようとする間 main が `Mutex` を持ち続ける場合（子は待たねばならない）
  - 8 スレッド × 1000 回の `Mutex` 経由の加算（臨界区間の中で `yield_now` する）
  - atomic の旗での空回り
  - それぞれがもう 1 本を spawn する 200 スレッド
  - スレッドのないプログラム

  変更前の main の zustc では、`flag` が時間切れで失敗する。
- tests/ui: 退行 0。前は通らなかった 2 件、`threads-sendsync/issue-4448.rs` と `threads-sendsync/send-resource.rs` が通るようになったので、ベースラインを更新した。
- `cargo test --release --workspace`（116 件通過）、`tests/compare.sh`（42 件 ok）、サイズ予算。
- KPI: pull request の `KPI` ワークフローのコメントに、ベースとの差がある。

## 残ったこと

- 並列には走らず、待たないスレッドは他のスレッドを走らせない。`thread_local!` は全スレッドで共有される。スレッドの中の panic は今もプロセスを止める（巻き戻しがない）ので、`join` が panic のために `Err` を返すことはない。
- 本物の OS スレッドはロードマップに残る。そのときに `Arc` が通すべき検査が `formal/tla/ArcDrop.tla` である。
