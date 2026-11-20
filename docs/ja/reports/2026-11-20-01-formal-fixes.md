# 形式モデルが見つけた欠陥の修正: 借用検査の打ち切り・Vec の桁あふれと割り当て失敗・キャッシュの一時ファイル

Date: 2026-11-20

[報告 2026-11-19-01](2026-11-19-01-formal-models.md) の欠陥 1〜4 を直した。どれも、直す前の失敗をテストかモデルで再現してから直した。修正は 1 つにつき 1 コミットに分け、KPI を 1 行ずつ記録した（`bench/kpi_log.md`）。欠陥 5（`std::thread::spawn` の即時実行）は設計の問題として残す。

## 1. 借用検査: 不動点を最後まで回す

`Bk::check` は worklist の回数が `50 * ブロック数 + 1000` を超えると、誤りなし（`None`）を返していた。

上限は、束の高さから導いた次の式に変えた。

- ブロックの入口の状態は join で大きくなるだけで、大きくなる回数は束の高さ H 以下である。
- H は、ムーブした場所（ムーブと未初期化の束縛から）、immutable で代入済みのローカル、holds の項目とそのローン、dead と escaped のローンの数の和である。
- worklist に入るのは、入口の状態が大きくなったときだけである。
- したがって回数は `n * (H + 1) + 1` を超えない。

これを超えることは起こりえないが、超えたら受け付けずに誤りにする（debug ビルドでは assert）。

- テストは `borrowck_props.rs` の `fixpoint_does_not_give_up`。長さ 700 の参照の連鎖を、イベント列として直接組み立てる。直す前は失敗した。
- `formal/repro/borrowck_rounds.py` は、K = 260 でも 1000 でも E0506 を見つけるようになった。

## 2. Vec の容量の桁あふれ

`Vec::reserve` を次のように変えた。

- `len + extra` が usize に入らないとき、または確保するブロックが isize::MAX バイトを超えるとき（std と同じ上限）は、`panic!("capacity overflow")` にする。
- 倍にする計算は飽和させる。
- この計算は総称でない関数 `__exr_vec_new_cap` に置く（要素の型ごとに検査・コンパイルしないため。KPI の節を参照）。

`Vec::<u64>::with_capacity(1 << 61)` の後に push すると、segmentation fault だったのが、rustc と同じく "capacity overflow" で終了コード 101 になった。テストは `tests/alloc_limits.rs` で、次の 3 つと、普通に伸びる場合を調べる。

- `with_capacity(1 << 61)`
- 要素のある Vec への `reserve(usize::MAX)`
- `reserve(usize::MAX / 4)`

## 3. 割り当ての失敗

prelude の `realloc` の呼び出し 12 箇所（Vec、整列の作業領域、HashMap、VecDeque など）を `__exr_realloc_ok` に替えた。

- null が返ったら、std の `handle_alloc_error` と同じく、メッセージ（"memory allocation failed"）を出して abort する。
- `realloc(p, 0)` は null を返しうるが、これは失敗ではないので扱わない。
- `Box` の malloc はコード生成が直接呼ぶので、今回は変えていない。

Kani のハーネスは、修正後のコードを写して調べる。`bug_*` を次の 3 つに替えた。

- `with_capacity_any_size`
- `reserve_any_size`
- `alloc_failure_stops`

どれも `#[kani::should_panic]` で、どんな大きさでも、また malloc が失敗しても、panic か abort で止まることを示す。Kani のポインタ検査（範囲外・null の参照）は panic として数えないので、範囲外に書く経路が 1 つでもあれば失敗する。

- 直す前の prelude では、3 つとも「panic 以外の失敗」で落ちることを確かめた。
- 中身の正しさは、これまでどおり `*_matches_model` が調べる。

## 4. キャッシュとスタンプの一時ファイル

`Cache2::save` は `<path>.tmp` に書いてから rename していた。一時ファイル名は全プロセスで同じだった。

- 名前を `<path>.<pid>.tmp` に変えた。`cache2.rs` は zustc 自身でもコンパイルするので、pid は libc の `getpid` で取る。
- 書き込みか rename に失敗したら、一時ファイルを消す。
- `src/stamp.rs` のスタンプの保存も同じやり方だったので、同じように直した。

`formal/tla/CacheSave.cfg` は今のコード（プロセスごとの名前）を検査し、全探索で成り立つ（72 状態）。前のやり方は `CacheSaveShared.cfg` として残し、反例（8 手）を出すことを `formal/run.sh` が確かめる。テストは `cache2.rs` の `save_tests::save_then_open`。

## 確かめたこと

- `cargo test --release --workspace` はすべて通った。
  - rust-src のない環境では、zust-analyzer の LSP テスト 2 件が直す前から落ちる。README の要件どおり rust-src を入れて回した。
- `tests/compare.sh` は 42 件すべて ok（valgrind あり）。
- tests/ui のベースライン検査は、退行 0。`--check` は crash 2 件（`parser/circular_modules_main.rs` などの循環モジュールでスタックあふれ）で失敗するが、変更前の zustc でも同じように落ちる。
- `formal/run.sh` は、TLA+ の 7 構成、`props` のテスト、Kani の 14 ハーネスがすべて期待どおりの結果になった。

KPI は `bench/kpi_log.md` の 2026-11-20 の 4 行にある。

- 1 行目は、変更の前（main + formal/）をこの機械で測り直した基準である。前の行とは機械が違うので、その差が出ている。
- 残りの 3 行が、修正 1〜3 それぞれの差分である。

| 変更 | no-op build | edit one fn | check | traits check | traits edit | crates no-op | crates edit | tests/ui (run / check / build / compile-fail) |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| 基準（0765679） | 0.9 M | 580.8 M | 3851.5 M | 1178.8 M | 61.0 M | 10.8 M | 316.2 M | 1698 / 1417 / 95 / 3327 |
| 1. 借用検査 | +0.00% | -0.09% | -0.06% | +0.03% | -0.01% | +0.00% | -0.03% | 1698 / 1417 / 95 / 3326 |
| 4. キャッシュ | +0.00% | +0.02% | -0.00% | -0.03% | +0.00% | +0.00% | +0.02% | 1698 / 1417 / 95 / 3327 |
| 2・3. Vec | +0.23% | -0.06% | +0.09% | -0.26% | +0.23% | +0.13% | +0.60% | 1698 / 1417 / 95 / 3327 |

- どれも判定の閾値（5%）より小さい。キャッシュの大きさは変わらない。
- Vec を使うプログラムは、2 つの検査と panic / abort の経路の分だけ約 540 バイト大きくなった。`bench/budget.txt` の `macros.cache_bytes` は、もともと余裕が 0.8%（67160 のうち 66655）しかなく、67398 になった。この項目だけ、`UPDATE_BUDGET=1` と同じ規則（現在値 + 5%）で 70768 にした。ほかの項目は CI の wild での大きさなので、そのままにしてある（どれも上限内）。
- compile-fail の 3327 / 3326 は、どの行でも 1 件が行き来している。キャッシュの修正（借用検査に関係しない）でも戻っているので、時間で揺れるテストである。ベースラインの検査は退行 0。
- Vec の修正は、最初は検査を `reserve` の中に書いた。`reserve` は要素の型ごとにインスタンスができるので、traits check が +2.65% になった。桁あふれの計算を総称でない関数 `__exr_vec_new_cap` に移すと、-0.26% になった。

## 残ったこと

- 欠陥 5: `std::thread::spawn` の即時実行。本物のスレッドを入れるなら、`ArcDrop.tla` の atomic 版のとおり `Arc` を変える必要がある。
- `Box` の割り当ての失敗。
- キャッシュのチェックサム。プロセスごとの一時ファイルで、混ざったファイルは読まれなくなった。それでも、ディスクの上で壊れたファイルを捨てるには要る。
