# トレイトオブジェクトが auto trait を持つようにする

Date: 2026-11-27

zustc の型はトレイトオブジェクトの `+ Send` / `+ Sync` を持たず（`ast_types::tysyn` が落としていた）、どの `dyn Trait` も
Send かつ Sync とみなしていた。そのため、`Rc` を持つ `Box<dyn Fn()>` が別のスレッドへ移れた（tests/security/send_dyn_thread.rs。
[レポート 2026-11-27-01](2026-11-27-01-native-threads.md) 以来の TODO）。このレポートでは、rustc と同じくそれを持たせる。

## rustc がすること

- `dyn Tr + Send` は Send で、`dyn Tr + Sync` は Sync である。ただの `dyn Tr` はどちらでもない。そのトレイトのどんな型も入りうるからである。
  トレイトがスーパートレイトとして持つ auto trait（`trait Tr: Send`）は持つ。
- 値を `dyn Tr + Send` にできるのは、その型が Send のときだけである（型強制の場所で E0277）。Sync も同じ。
- 型強制は auto trait を外せる（`Box<dyn Tr + Send>` → `Box<dyn Tr>`）が、足すことはできない（E0308）。
- auto trait の順番と重複は効かない。`dyn Tr + Send + Send` は `dyn Tr + Send` である。

## 変えたこと

- `ast_types::tysyn` は、オブジェクトの文字列に、トレイトの後ろに auto trait を `Send`、`Sync` の順に書く（`Fn() -> i32 + Send + Sync`）。
  重複したものは 1 回だけ書くので、`dyn Tr + Send + Send` は同じ型になる。`types::dyn_autos` はその文字列をトレイトと 2 つのフラグに分ける。
  auto trait だけのオブジェクト（`Box<dyn Send>`、`&dyn Sync`）は最初のものをトレイトとする。`dyn Sync + Send` は `Send + Sync` で、両方を持つ。
- 文字列からトレイトを読むところは、auto trait を除いたものを使う。vtable（`to_dyn`）、メソッド呼び出し（`dyn_trait_args`、`dyn_methods`、
  推論のメソッド探索）、`Fn` の呼び出し（`dyn_fn_sig`）、`dyn Fn` の呼び出しのリージョン検査（`rg_dyn_fn_args`。`0 as &dyn ..` を
  パースするので、`+ Send` が足し算として読まれてしまう）、そして `Box<dyn Error>` と同じく prelude の `__DynError` になる
  `Box<dyn Error + Send + Sync>`。そのほかのところ（`implements`、トレイトソルバの auto trait、`impl dyn Tr` の探索）は、すでに
  文字列を `+` で分けていた。
- auto trait を判定する 3 か所（`Program::auto_trait`、トレイトソルバの `auto_rec`、acg の `auto_trait_fails`）は、`dyn` 型を、
  書かれた auto trait かトレイトのスーパートレイトによってだけ Send / Sync とする。
- 値がオブジェクトになるところで vtable を作る `to_dyn` は、値の型をオブジェクトの auto trait と照らし合わせる。オブジェクトのトレイトそのものである
  auto trait（`dyn Send`）とも照らし合わせる。E0277 で、それを満たさない部分を、クロージャのキャプチャとプログラム自身の構造体をたどって名指す
  （それをキャプチャするクロージャでも、`Rc` の中のポインタでもなく `Rc<i32>`）。定数の初期化式の中では、そのエラーは定数の行に出る（`zustc check` でも）。
- `TraitDb::gives_auto` は、答えを未定とする前に、トレイトのスーパートレイトをすべて見る。`trait Tr: A + Error` と `trait A: Send` なら、zustc が
  `Error` のスーパートレイトを知らなくても `dyn Tr` は Send である。
- `dyn Tr + Send` の型のハッシュは、`dyn Tr` のものと同じく `Tr` のメソッドを含む。
- オブジェクトの `Box`、`&`、`*const` から、auto trait のより少ないものへの型強制は同じ値である。より多いものへは、これまでどおり型の不一致（E0308）である。
  それを決めるのは `dyn_coerces` である。オブジェクトのトレイトがスーパートレイトとして持つ auto trait は、持つものと数える（`trait Job: Send` の
  `Box<dyn Job>` は `Box<dyn Job + Send>` である）。auto trait だけのオブジェクトは、その集合とみなす（`Box<dyn Send + Sync>` は `Box<dyn Sync>` である）。
- ポインタのキャストは、オブジェクトの auto trait を外せるが足せない。`*const dyn Job as *const (dyn Job + Send)` は rustc と同じく E0804 である
  （cast/ptr-to-trait-obj-add-auto.rs が通る）。rustc と同じく、これは主トレイトのあるオブジェクトについてである。auto trait だけのオブジェクトには
  メソッドがなく、`*const dyn Sync as *const (dyn Send + Sync)` は受け付ける（tests/security/send_dyn_ptr_cast_auto_only.rs。前は
  `dyn Send` から `dyn Send + Sync` を拒んでいた）。
- `&(dyn Tr + Send)` → `*const dyn Tr`（参照から生ポインタへの型強制と `as` のキャスト）も、同じ `dyn_coerces` で auto trait を外せる
  （tests/security/send_dyn_ref_to_ptr_drop_auto.rs）。
- フィールドを持つプログラム自身の構造体は、型引数ではなくフィールドで判定する（`fn() -> T` のフィールドの `Maker<Rc<u32>>` は Send）。
- `static` の型は Sync でなければならない（E0277）。この検査を、プログラムのトレイトのスーパートレイトの auto trait を読んだ後に移した。
  そのため、`trait Stage: Sync` の `&dyn Stage` は Sync になる（consts/issue-54582.rs）。

## 確かめたこと

- tests/security: send_dyn_thread.rs が通る（もう TODO ではない）。新しいケースは send_rc_dyn_send_coerce.rs（`Box<dyn Fn() + Send>` にする
  `Rc` のクロージャ）、sync_cell_dyn_sync_coerce.rs（`&(dyn Fn() + Sync)` にする `Cell` のクロージャ）、send_rc_bare_dyn_send.rs（`Box<dyn Send>` にする `Rc`）、send_dyn_ptr_cast_add_auto.rs（E0804）、send_dyn_add_auto.rs
  （`Box<dyn Fn()>` から `Box<dyn Fn() + Send>`、E0308）。どれも rustc が拒否する場所で拒否される。
- rustc と比べた。`Box<dyn Tr + Send>`（とその `Vec`）はスレッドへ移れる。`Box<dyn Tr>` は移れず、`T: Send` の境界も満たさない。
  `Mutex<Box<dyn Tr + Send>>` は動く。`?`、`Box::new(e)`、`"msg".into()` での `Box<dyn Error + Send + Sync>` は前と同じく動く。
- rustc の UI テスト：後退は 0、新たに通るのは 5 つ（coercion/retslot-cast.rs、mismatched_types/trait-bounds-cant-coerce.rs、
  traits/bound/sugar.rs、traits/issue-7013.rs、cast/ptr-to-trait-obj-add-auto.rs）。
- `cargo test --release --workspace`。

## 残っていること

- auto trait を持つオブジェクトへの impl（`impl M for dyn Obj + Send`）は、トレイトだけで見つかる。そのため、`dyn Obj` でのそのメソッドの
  呼び出しは型の不一致（E0308）として拒否される。rustc は E0599 を報告する。
- `Arc::new(x)` から作る `Arc<dyn Tr>` / `Rc<dyn Tr>` は、auto trait があってもなくても扱えない。
- オブジェクトの中の `Unpin`、`UnwindSafe`、`RefUnwindSafe` は、これまでどおり落とす。
