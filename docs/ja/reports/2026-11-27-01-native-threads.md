# `std::thread` を OS スレッドに

Date: 2026-11-27

[報告 2026-11-20-02](2026-11-20-02-green-threads.md) 以来、zustc のスレッドはグリーンスレッドだった。OS スレッドは 1 本で、spawn したスレッドごとにスタックを持ち、スレッドが待つところで切り替える。これが健全だったのは、2 つの切り替え点の間で他のものが走らなかったからにすぎない。そのため `Arc` は `Rc` と同じで、atomic はただのセルで、並列性はなかった。待たないスレッドは他のスレッドを走らせなかった。この報告では、それを Linux の std と同じ OS スレッドに置き換える（issue #8 の「Threads」）。

## Linux の std がしていること

これらの型の std の実行環境を `library/std/src/sys`（rustc 1.97）で読んだ。

- `thread::spawn` は 2 MiB のスタック（`RUST_MIN_STACK` で変わる）で `pthread_create` を呼ぶ。クロージャの値は、スレッドとその `JoinHandle` が共有するパケット（`Arc`）に入る。`join` は `pthread_join` で、その後で値を取り出す。`join` せずに drop された `JoinHandle` はスレッドを detach する。
- `Thread` は、id・名前・parker の `Arc` である。ThreadId はスレッドを作った順に 1 から数えるので、main スレッドは 1 になる。
- parker、`Mutex`、`Condvar`、`RwLock`、`Once` は futex の上の状態機械である。
  - parker の状態は empty / notified / parked。
  - `Mutex` は 0 がロックなし、1 がロック中、2 が待つスレッドのいるロック中。`lock` は 0 -> 1 で取るか、少し回ってから 2 にして futex で待つ。`unlock` は 0 にし、2 だったなら待つスレッドを 1 つ起こす。
  - `Condvar` は notify のたびに増えるカウンタで、`wait` は Mutex を放してからそれを待つ。
  - `Once` は incomplete / running / running with waiters / complete。
- `Arc` はアトミック命令で数える。clone は 1 足し（Relaxed。`isize::MAX` を超えると abort）、drop は 1 引き（Release）、最後の 1 を引いた drop は fence（Acquire）してから解放する。formal/tla/ArcDrop.tla はこれをモデル化している。
- spawn したスレッドの panic は `thread '<unnamed>' panicked at ..` を出し、そのスレッドを巻き戻し（その値は drop され、持っていた `Mutex` は poison される）、スレッドローカルの drop を走らせる。`join` はペイロードを持った `Err` を返す（その Debug は `Any { .. }`）。`RUST_BACKTRACE` についての note は最初の panic にだけ付く。
- `println!` は行全体の間 stdout のロックを持つので、2 つのスレッドの行が互いに割り込むことはない。

## 変えたこと

### アトミック命令（`crates/ir`、minclif、Cranelift、wasm）

IR に命令を 5 つ足した。どれも sequentially consistent である（std の ordering はどれもこれと同じか弱い）。`AtomicLoad`、`AtomicStore`、`AtomicRmw`（exchange、add）、`AtomicCas`、`Fence` で、1、2、4、8 バイトの整数に使う。ほかの read-modify-write（`fetch_and`、`fetch_max` など）は prelude の中の compare-and-swap のループである。

- minclif: load は `mov`（揃った x86-64 の load はアトミックで、acquire になる）、store と exchange は `xchg`、それに `lock xadd`、`lock cmpxchg`、`mfence`。使うのは rax、rcx、rdx で、minclif がこれらを割り当てることはない。これらが読むアドレスは load でも store でもない使い方と数えるので、その後ろのスタックスロットが変数にされることはない。
- Cranelift: `atomic_load`、`atomic_store`、`atomic_rmw`、`atomic_cas`、`fence`。
- wasm32: ただの load と store（そこではスレッドは 1 本）。

prelude はこれらを関数として名指す（`__exr_atomic_load`、`_store`、`_xchg`、`_xadd`、`_cas`、`_fence`）。acg はその呼び出しを命令に下ろす（`crates/typeck/src/acg/p2_call.rs` の `atomic_intrinsic`）。そのため、codegen のほかの部分は atomic のことを知らない。

### prelude（`crates/syntax/src/prelude_lib.rs.in`）

- atomic の型（`AtomicUsize` .. `AtomicI64`、`AtomicBool`、`AtomicPtr`）は intrinsic を使う。`fence` と `compiler_fence` もある。atomic の `load` で回って他のグリーンスレッドを走らせていた部分はなくなった。
- `Arc` は `xadd` で数える。`try_unwrap` は 1 -> 0 の compare-and-swap である。`into_inner` は std と同じ（最後の 2 つをそれぞれ手放す 2 つのスレッドのうち、1 つが値を得る）。
- `spawn`、`join`、`JoinHandle` の drop（detach）、パケット、`Thread`（参照カウントを持つようにした）、`current`、`park`、`unpark` は std と同じで、`pthread_create` / `pthread_join` / `pthread_detach` と、libc の `syscall` を通した `futex(2)` の上にある。
- `Mutex`、`Condvar`、`Once` は std の futex のもの。`RwLock` は std のものより単純で、状態は reader の数か、writer なら `u32::MAX`。取れないスレッドは、unlock のたびに増える 2 つ目の futex の語で待つ。`OnceLock` は `Once` と値である。
- `mpsc` は futex の Mutex の下のキューと、両端が待つ Condvar である。`recv` は送り手がすべて drop されると `Err` を返す（前は、全スレッドが待ちになったとき）。`send` は受け手がいなくなると失敗する。`sync_channel(n)` の send はキューが一杯の間待ち、上限 0 なら自分の値が受け取られるまで待つ。
- `thread_local!`（`lexer::expand_thread_local`）はスレッドごとに 1 つの値を持つ。最初に使われたときに番号の付く鍵と、各スレッドのコンテキストの中の値の列である（コンテキストは pthread のキーが持つ）。spawn したスレッドは、終わるときに自分の値を新しいものから drop する。これは `join` が戻る前に起きる。
- spawn したスレッドの panic: メッセージはそれを `<unnamed>` と呼び、note は 1 回だけ付く。その後スレッドはパケットに panic したと印を付け、スレッドローカルを drop し、`pthread_exit` で終わる（オーバーフローのトラップの SIGILL ハンドラからも）。`join` は `Err(Any { .. })` を返す。
- `print!` / `println!` は、スレッドが spawn された後は行の間ロックを取る。このロックは再入可能で、`StdoutLock` は std と同じく生きている間それを持つ。
- panic・切断・ロックが端で何をするかは std と同じにした（プルリクエストのレビューから）。
  - thread_local! の値は、そのスレッドが作った順の逆に drop される。drop の始まった値には二度と届かない（`with` は panic し、`try_with` は `AccessError` を返す）。ほかの値の drop が初めて作った値も drop される。panic する drop はプログラムを abort する（"thread local panicked on drop"）。
  - `Once` のクロージャを走らせている間に panic した spawn したスレッドは、その `Once` を poison するので、待っているスレッドは起きる。その後 `call_once` は panic し、`call_once_force` はもう一度走って `is_poisoned()` を見る。`OnceLock` は次の呼び出し手の初期化を走らせる。そのスレッドが持っていた `StdoutLock` は放される。
  - 受け手が値を取る前にいなくなった `sync_channel(0)` の send は、その値と一緒に失敗する。drop された受け手は、キューに残った値を drop する。チャネルのエラーは std の `SendError`（その Debug は `T: Debug` を要らない）、`RecvError`、`TryRecvError` である。
  - `RwLock` の Debug は待たずに読みロックを取り、writer が持っている間は `<locked>` と出す。
  - `spawn(f)` の前にローカルに置かれたクロージャも、呼び出しの中に書かれたものと同じく調べる。参照で持つキャプチャは E0373、`Send` でないものは E0277 である（tests/security/spawn_named_closure_borrow.rs、send_rc_named_closure_thread.rs）。`StdoutLock` は `Send` ではない（send_stdout_lock_thread.rs）。
  - `try_recv` は、キューが空だと分かったのと同じロックの下で Empty と Disconnected を見分ける。
  - `pthread_create` が失敗したときは、渡すはずだったものを解放する。`pthread_key_create` の失敗は `spawn` の中の panic で、新しいスレッドでの `pthread_setspecific` の失敗は abort である。

panic と print の経路のうちスレッドを意識する部分は、1 つのフック（`__EXR_THREAD_HOOK`）を通る。これは最初の spawn が設定する。スレッドのないプログラムにはフックがなく、前と同じことをする。そのプログラムが払うのは、static 1 つと、panic と print の関数の中でそれを調べることだけである。prelude は今も名前で絞られるので、何も spawn しないプログラムには pthread の関数は入らない。新しい型のうち 2 つ（`__TlsKey`、`__ThreadPanic`）は `crates/syntax/src/prelude.rs` で impl の持ち主として並べたので、その impl はその型と一緒にだけ入る。

## 確かめたこと

- `tests/threads.rs`（元の `tests/green_threads.rs`）は、`tests/threads/` の各プログラムを 2 つのバックエンドでビルドし、出力を rustc のものと比べる。3 つが新しい。
  - `parallel`: 8 スレッド × 100 000 回の atomic の加算（それぞれ `Arc` の clone を通して）、`Mutex` の下での 8 × 20 000 回の read-modify-write、4 つの生産者 × 10 000 回の send、`sync_channel(0)`、`RwLock` の reader と writer、`Condvar` で待つ消費者、8 スレッドからの `Once` と `OnceLock`、park / unpark、detach したスレッド。どの合計も、すべての操作がアトミックなときにだけ正確になる。バックエンドごとに 5 回走らせる。
  - `panic`: spawn したスレッドでの範囲外の添字、オーバーフローのトラップ、`panic!`。どれもそのスレッドだけを終わらせ、その `join` は `Err` を返し、スレッドローカルは drop され、note は 1 回だけ付く。
  - `tls`: 各スレッド自身の `thread_local!` の値と、スレッドが終わるときのその drop。
  - `edges`: 上の一覧の場合を、rustc のビルドが出すとおりに。
- デッドロックのプログラム（再ロック、何も unpark しない park、自分のクロージャの中の `call_once`、何も notify しない Condvar）は、std と同じく永久に待つようになった。テストは、それぞれが 2 秒後にもまだ走っていて、rustc が出さなかったものを何も出していないことを確かめる。前は "deadlock" で panic していた。
- `tests/probe/sync_once.rs` は、3 つのスレッドのどれが `call_once` を走らせたかを出していた。スレッドが並列に走ると、それはスケジューラ次第である（rustc でも同じ）。そこで、1 つが走らせたことを出すようにした。
- `cargo test --release --workspace`。唯一の失敗である `ast_codegen` の `std_file` は main でも起きる。rustc 1.97 は `OpenOptions` を別の順で調べるので、メッセージが違う。
- CPU を使うループ（スレッドごとに xorshift を 2 億回、`-O`）は、この 4 コアのマシンで 1 スレッドが 0.42 秒、4 スレッドが 0.43 秒。グリーンスレッドでは 4 スレッドが 1.63 秒で、1 スレッドの 4 倍かかった。

## コスト

- すべてのプログラム: panic の経路がフックを調べ、`println!` の関数も調べる。`hello` のバイナリは 112 バイト大きくなり（9 704 -> 9 816）、キャッシュは 349 バイト大きくなった（8 191 -> 8 540）。`bench/budget.txt` の `float_fuzz` には 0.1 % の余裕しか残っていなかったので、そのキャッシュと `options`・`text_more` のキャッシュの予算を上げた（`UPDATE_BUDGET` と同じく、計測値 + 5 % に）。
- `Arc` を使うプログラム: clone と drop が、ただの加算ではなく `lock xadd` になる。
- prelude の長さは前と同じ（16 591 行、3 行減）。atomic の型が縮んだ分だけ、実行環境が増えた。

## 残っていること

TODO.md の「Threads」:

- Mutex と RwLock の poisoning はなく、panic したスレッドの値の drop もない。どちらも巻き戻しが要る。
- `thread::scope`、`Condvar::wait_timeout`、`Mutex::try_lock` はない。（`Builder`、`park_timeout`、`recv_timeout`、
  `Barrier` はその後、[レポート 2026-11-27-02](2026-11-27-02-threads-ui.md) で入った。）
- main スレッドのスレッドローカルは、終了時に drop されない。
- `unwrap` が出す場所は 1 つの static で、その間に他のスレッドが上書きしうる。
- wasm32 には `spawn` がない（ホストに pthread がない）。そこでは atomic とロックはただのアクセスとして動く。
