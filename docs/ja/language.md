# 対応している言語のサブセット

zustc がコンパイルできるのは Rust のサブセット。std を使うプログラム (Vec / String / HashMap / 書式化 / イテレータ / Box /
Rc など) は、zustc が持つ組み込みの prelude (`crates/syntax/src/prelude_lib.rs.in`。本物の std ではない) で動かす。
`#![no_std]` のプログラムも扱える。

正しさは、同じプログラムを rustc でもビルドし、実行結果 (標準出力と終了コード) を比べて確かめている
(`tests/compare.sh` は `examples/*.rs`、`tests/probe.sh` は `tests/probe/*.rs`)。no_std のものは rustc 側を libcore だけ
(`-C panic=abort`) でビルドする。

**大事な前提**: 借用検査・可変性の検査・ムーブの検査・match の網羅性の検査はしない (下の「型推論と検査」)。rustc を通る
プログラムを速くビルドするための道具で、rustc の代わりに正しさを確かめるものではない。

## 言語

- アイテム：
  - `fn`
  - `extern "C"` の宣言と定義
  - `struct`：名前付き / タプル / unit
  - `enum`：unit / tuple / struct のバリアントと、判別値
  - `const`、`static`、`static mut`
  - `use`（名前は最後のセグメントで解決する）
  - クレート属性（`#![no_std]` など）
- impl と trait：
  - 固有 `impl`
  - `trait`（必須メソッドとデフォルトメソッド）
  - `impl Trait for S`：struct / enum / プリミティブ型（`impl Tr for u32`）
  - `#[derive(Clone, Copy, PartialEq, ..)]`
- ジェネリクス：
  - 関数（`<T: A + B>`、`where`、turbofish）
  - struct / enum（`Pair<T>`、`Option<T>`）
  - `impl<T> X<T>`、`impl<T> Option<&T>` のような self 型パターン
  - すべて呼び出し元で型引数を推論し、単相化する。ライフタイムは消去する
  - 引数位置の `impl Trait`（名前のない型パラメータとして扱う）
- クロージャ（`examples/closures.rs`）：
  - キャプチャ：参照（既定）と `move`（値）。入れ子のクロージャ、`self` やメソッド呼び出しを含む本体
  - `F: Fn(A) -> B` / `FnMut` / `FnOnce` 境界、`impl Fn(A) -> B` 引数、`&F` / `&mut F` 渡し
  - 戻り値位置の `impl Fn(A) -> B`（末尾が `move` クロージャで、キャプチャが引数だけのもの）
  - struct のフィールドに入れる（`struct Button<F: Fn(i64) -> i64> { on_click: F }`、`(self.on_click)(x)`）
  - 引数・戻り値の型は、注釈 → 呼び先の境界 → 使われ方（親の型推論）→ 本体の型推論の順に決める
- `dyn Trait`（`examples/dyn_trait.rs`）：
  - `&dyn Trait` / `&mut dyn Trait`。`&T` からの暗黙の変換（引数・let・フィールド・配列・戻り値）と `as &dyn Trait`
  - デフォルトメソッド、ジェネリックな impl（`impl<T> Tr for W<T>`）、プリミティブ型への impl を vtable 経由で呼ぶ
  - `&dyn Fn(A) -> B`（クロージャを vtable 経由で呼ぶ）
  - auto trait: `dyn Trait + Send` / `+ Sync` は rustc と同じくそれを持つ。値をそれにするには、その型がそれを持たなければならない（E0277）。
    ただの `dyn Trait` は、トレイトがスーパートレイトとして持つ（`trait Tr: Send`）のでなければ Send でも Sync でもない。
    型強制はそれを外せる（`Box<dyn Tr + Send>` → `Box<dyn Tr>`）が、足すことはできない（E0308）
- 関数ポインタ（`examples/fn_ptr_box.rs`）：
  - `fn(A, B) -> R` 型。関数アイテムを値として使う（引数・戻り値・let・struct のフィールド・配列）、`(op.f)(a, b)`
  - `F: Fn(A) -> B` に関数を渡す（`apply(double, 5)`、`o.map(double)`）
  - ジェネリック関数は turbofish（`id::<i64>`）か期待型（`let f: fn(i64) -> i64 = id;`）で型引数を決める
  - `&dyn Fn` / `Box<dyn Fn>` にする（引数 3 個まで。prelude の中継関数 `__exr_fnptrN` が vtable に入る）
- `Box<T>`（`examples/fn_ptr_box.rs`）：
  - `Box::new(x)` は libc の `malloc` でヒープに置き、drop で `free` する
  - `Box<dyn Trait>` / `Box<dyn Fn(A) -> B>`、自動 deref、`*b`、`&Box<T>` → `&T`、`Option<Box<Node>>` のような再帰的な型
- `Vec<T>` / `String`（`examples/vec_string.rs`）：prelude に Rust で書いてある（`crates/syntax/src/prelude_lib.rs.in`）。libc の `realloc` / `free` を使い、Drop で要素を drop してから解放する
  - `Vec`：`new` / `with_capacity` / `push` / `pop` / `len` / `is_empty` / `capacity` / `get(_mut)` / `first` / `last` / `insert` / `remove` / `swap_remove` / `swap` / `reverse` / `truncate` / `clear` / `append` / `as_slice` / `as_mut_slice` / `extend_from_slice` / `clone`（`T: Copy`）/ `contains` / `==`（`T: PartialEq`）/ `sort`（`T: Ord`、挿入ソート）
  - `vec![a, b]` / `vec![x; n]`、添字 `v[i]`（範囲外は panic）、`&v[a..b]`、`for x in &v` / `v.iter_mut()` / `v`（値で回す）
  - `&Vec<T>` → `&[T]`、`&String` → `&str` の暗黙の変換（ジェネリックな `&[T]` 引数にも）
  - `String`：`new` / `from` / `with_capacity` / `push_str` / `push(char)`（UTF-8 に符号化）/ `len` / `is_empty` / `as_str` / `as_bytes` / `clear` / `truncate` / `clone` / `==`（`String` 同士と `&str`）、`"..".to_string()` / `to_owned()`
  - `&str` / `&[T]` の `==` は中身で比べる
  - Vec / String のメソッドは、型の名前がコードに現れたときだけ prelude から付ける（自分の struct に `push` があるだけでは付かない）
- Iterator（`examples/iterators.rs`）：prelude に `trait Iterator { type Item; fn next(..); .. }` とアダプタを Rust で書いてある
  - 関連型：`type Item = u32;`（impl）と `Self::Item` / `I::Item`（シグネチャ）、`I: Iterator<Item = T>` / `where Self: Iterator<Item = &T>` の制約で型パラメータを解く
  - 自前の `impl Iterator for S`、`for x in it`（`while let Some(x) = it.next()` にする）
  - 作るもの：`v.iter()` / `iter_mut()` / `into_iter()`（Vec・配列・スライス）、`(a..b)` / `(a..=b)` を値として、`s.chars()`（UTF-8 を復号）
  - アダプタ：`map` / `filter` / `filter_map` / `enumerate` / `zip` / `take` / `skip` / `take_while` / `chain` / `copied` / `cloned`
  - 消費：`collect`（`Vec<_>` / `String`、`collect::<Vec<_>>()` と `let v: Vec<_> = ..` の `_` も）/ `sum` / `product` / `count` / `fold` / `for_each` / `any` / `all` / `find` / `position` / `last` / `nth` / `max` / `min`
  - クロージャの引数の型は、呼び先の境界（`F: FnMut(Self::Item) -> B`）を関連型まで解いて決める。型推論も関連型を辿る（`let s: i64 = (1..=10).sum();` の範囲は i64）
  - prelude のイテレータ部品は、イテレータを作ってアダプタ・消費メソッドを呼ぶコード（`v.iter().map(` / `(0..n).sum(` / `impl Iterator` / `.chars()`）があるときだけ付ける。`for x in v.iter()` だけ、`Option::map` だけ、変数名の `count` だけでは付けない
- 書式化（`examples/formatting.rs`）：`format!` / `print!` / `println!` / `eprint!` / `eprintln!` / `write!` / `writeln!`（`String` と `fmt::Formatter` に）
  - `{}` / `{:?}` / `{:#?}`、位置・名前つき引数、`{x}` の取り込み、`{{` / `}}`
  - 幅・寄せ・埋め（`{:>8}` / `{:*^9}` / `{:08}` / `{:>w$}`）、`+`、`#`、精度（文字列の切り詰め）、`x` / `X` / `b` / `o`
  - `impl fmt::Display for S`（`fmt::Result` / `fmt::Formatter<'_>` / `f.pad`）と `x.to_string()`
  - `#[derive(Debug)]`（struct・タプル構造体・unit 構造体・enum・ジェネリック）。整数・bool・char・`&str`・String・Vec・配列・スライス・タプル・Option・Result・HashMap の Debug
  - 型ごとの Display / Debug の本体は codegen が作る（`__exr_display::<T>` / `__exr_debug::<T>`）。prelude は Rust で書いた helper だけ
- HashMap（`examples/hashmap.rs`、`examples/text_more.rs`）：prelude に Rust で書いた（オープンアドレス法・線形探索・FNV-1a）
  - `new` / `with_capacity` / `insert` / `get` / `get_mut` / `remove` / `contains_key` / `len` / `is_empty` / `clear` / `m[&k]`
  - `entry(k).or_insert(v)` / `or_insert_with` / `and_modify`
  - `iter` / `iter_mut` / `keys` / `values` / `values_mut` / `into_iter` / `for (k, v) in &m`、`collect::<HashMap<_, _>>()`
  - キーは整数・bool・char・`&str`・String と `#[derive(Hash)]` の struct / enum / タプル（ハッシュ関数は型ごとに codegen が作る）
  - 回る順番は rustc と違う（rustc もランダムなので、例ではキーを並べ替えてから出す）
- 文字列と char（`examples/strings_chars.rs`）：prelude の `__exr_str_<名前>` / `__exr_char_<名前>` があれば、
  `s.名前(..)` / `c.名前(..)` はそれを呼ぶ（型推論もそのシグネチャを使う。&str の引数には char も渡せる）
  - `to_uppercase` / `to_lowercase` / `replace` / `replacen` / `repeat` / `split("..")` / `split_once` / `strip_prefix` / `strip_suffix` /
    `trim_start` / `trim_end` / `rfind` / `char_indices` / `bytes` / `eq_ignore_ascii_case` / `parse::<整数>()`（`ParseIntError` の Display / Debug も std と同じ）
  - String の `insert` / `insert_str` / `remove` / `pop` / `retain` / `+` / `+=`、`&s[a..b]`（文字の境界でなければ panic）
  - char の `is_alphabetic` / `is_numeric` / `is_alphanumeric` / `is_whitespace` / `is_uppercase` / `is_lowercase` / `to_digit` / `is_digit` /
    `to_uppercase` / `to_lowercase` / `len_utf8`、`char::from_digit` / `char::from_u32` / `char::from(u8)`。
    Unicode の表は持たないので、ASCII・Latin-1・ギリシャ文字・キリル文字・仮名・漢字・ハングルの範囲で近似する
- イテレータの追加分（`examples/iter_vec_more.rs`）：`rev`（Vec・配列・範囲・chars・map / filter / copied / chain の後。DoubleEndedIterator の代わりに各型が `next_back` を持つ）/
  `step_by` / `peekable`（`peek` / `next_if`）/ `skip_while` / `scan` / `inspect` / `flat_map` / `flatten`（中身は Vec・Option・イテレータ）
- Vec / スライスの追加分：`retain` / `retain_mut` / `dedup` / `dedup_by_key` / `drain(範囲)` / `split_off` / `resize` / `fill` / `first_mut` / `last_mut` /
  `windows` / `chunks` / `to_vec` / `join` / `concat`（要素が `&str` か String）
- Option / Result の追加分：`expect` / `zip` / `map_or_else` / `or_else` / `and` / `get_or_insert_with` / `insert` / `is_none_or` / `is_ok_and` / `is_err_and` / `map_or`
- clone（`examples/clone_derive.rs`）：`#[derive(Clone)]` の struct / enum / ジェネリック、Box・タプル・配列・Option・Result、手書きの `impl Clone`。
  本体は型ごとに codegen が作る（`__exr_clone::<T>`。drop の要らないフィールドはコピー、ほかはフィールドごとに clone）
- Default（`examples/defaults.rs`）：`#[derive(Default)]`、手書きの `impl Default`、`Default::default()` / `T::default()` / `i32::default()` / `Vec::default()`、
  構造体の更新記法 `S { a: 1, ..Default::default() }` / `..base`、`unwrap_or_default`、`std::mem::take` / `swap` / `replace`（Copy でない型も）。
  本体は型ごとに codegen が作る（`__exr_default::<T>`: 0 で埋め、手書きの impl を持つフィールドはそれを呼ぶ）
- コレクション（`examples/collections.rs`）：`HashSet`（HashMap<T, ()> の上に。和・積・差・collect）、`BTreeMap` / `BTreeSet`（キーの順に並べた Vec と二分探索。
  回る順・`entry().or_insert` / `or_default`・`first_key_value`・`pop_first`・`m[&k]` は std と同じ）、`VecDeque`（リングバッファ。`d[i]`・両端の push / pop）。
  キーが `&str` / String の map の `get("k")` は std の Borrow と同じく `&str` で引ける
- スレッド（`tests/threads/`）：`thread::spawn` / `join` / `is_finished` / `yield_now` / `sleep` / `current` / `park` /
  `park_timeout` / `Thread::unpark`、`thread::Builder`（`name`、`stack_size`、`spawn`）、`mpsc` のチャネル（`channel`、`sync_channel`。
  `recv_timeout`、`Receiver` の `into_iter`）、`Mutex`、`Condvar`、`RwLock`、`Barrier`、`Once`、`OnceLock`、
  atomic、`Arc`、`thread_local!`。std と同じく OS スレッド（pthread、スタックは既定で 2 MiB）で、並列に走る。atomic と `Arc` の
  カウントはアトミック命令で、ロックは std と同じ状態遷移で futex の上で待つ。何も終わらせられない待ちは、std と同じく
  永久に待つ。spawn したスレッドの panic は `thread '<unnamed>' panicked at ..`（Builder で名前を付けたスレッドならその名前）を出し、そのスレッドの `thread_local!` の値を
  drop して、そのスレッドだけを終わらせる。その `join` は `Err`（`Any { .. }`）を返す。巻き戻しはないので、そのスレッドの
  ほかの値は drop されず、持っていた Mutex や RwLock はロックされたまま残る（poisoning はない。走らせていた `Once` は std と同じく poison される）。スレッドが終わるときの `thread_local!` の値の drop での panic は、代わりにプログラムを abort する（std の "thread local panicked on drop"）。`join` はそれから回復できない
- `impl Index` / `IndexMut`：`x[i]` は x が `index` / `index_mut` を持つ struct なら `*x.index_mut(i)` にする
- 関連定数（`examples/assoc_consts.rs`）：`impl S { const N: usize = 4; }` / trait の `const K: u32;`（既定値も）。`S::N` / `Self::N` / `T::N`（型引数）、配列の長さにも
- モジュール（`examples/modules.rs`）：1 ファイルの中の `mod m { .. }`（入れ子・`pub(crate)`・`use self:: / super:: / crate::`・`m::f()` / `m::S` / `m::N`）。
  字句解析の前に `mod m {` と対応する `}` を空白にして平らにし（位置と行は変えない）、名前解決はパスの先頭のモジュール名を読み飛ばす。
  `#[cfg(test)] mod tests { .. }` は消す
- `macro_rules!`（`examples/macros.rs`）：字句解析の前にテキストとして展開する（展開は 1 行に収め、後ろの行番号を変えない）。
  断片 expr / ident / ty / literal / tt / block など、繰り返し `$(..),*` / `$(..)+` / `$(..)?`、再帰、アイテムを作るマクロ。
  式の断片は `( .. )` で包んで埋め込むので `square!(1 + 2)` の優先順位は Rust と同じ
- 演算子のオーバーロード：`impl Add / Sub / Mul / Div / Rem / Neg / Not / BitAnd / BitOr / BitXor / AddAssign ..`。
  `a + b` は左辺が struct で `add` メソッドがあれば `a.add(b)` にする（`impl Mul<i32> for V` のような右辺の型も）
- `&str` のメソッド：`split(char)` / `split_whitespace` / `lines` / `trim*` / `starts_with` / `ends_with` / `contains` / `find`（引数は `&str` か `char`）、`<` などの比較と `sort`
- 大小比較（`examples/ordering.rs`、`examples/ordering_impl.rs`、`examples/ordering_inline.rs`）：
  - `<` / `<=` / `>` / `>=` をタプル・配列・スライス・`&str`・String・Vec・Option・Result と `#[derive(PartialOrd, Ord)]` の struct / enum に（辞書順。enum はバリアントの判別値の順）
  - 手書きの `impl PartialOrd` / `impl Ord`（`impl PartialEq` / `impl Eq for S {}` も）。`<` と sort はそれを呼ぶ
    - sort / sort_unstable / max / min / clamp / BinaryHeap / BTreeMap / BTreeSet / Reverse / 境界 `T: Ord` / `T: Eq`、タプルや Option の中、enum への impl も
    - 配列・スライス・Vec・参照の `==` の要素も手書きの eq を呼ぶ
    - `BTreeSet` / `BTreeMap` の collect は std と同じく、等しいものが続いたら後のものを残す（insert は前のキーを残す）
  - `cmp` / `partial_cmp`（整数にも）、`Ordering` と `reverse` / `then` / `then_with` / `is_lt` など、`std::cmp::Reverse`
  - `max` / `min`（Ord な struct・タプル・String にも）、`std::cmp::max` / `min`
  - `sort` / `sort_by` / `sort_by_key` / `sort_by_cached_key` / `sort_unstable*`（安定なマージソート。Vec・配列・スライス・`v[a..b]`）、`binary_search` / `binary_search_by` / `binary_search_by_key`（同じ値が並ぶときに返す位置も std と同じ）、`clamp`（整数・Ord な型。`lo > hi` なら panic）、イテレータの `max` / `min` / `max_by_key` / `min_by_key` / `max_by` / `min_by`
  - `BinaryHeap`（`examples/binary_heap.rs`）：`new` / `with_capacity` / `from(vec)` / `collect` / `push` / `pop` / `peek` / `len` / `is_empty` / `clear` / `iter` / `into_iter` / `into_vec` / `into_sorted_vec` / `{:?}`。
    sift の手順を std と同じにしてあるので、`iter` / `into_vec` / `{:?}` の並びも rustc と一致する。`Reverse` で最小ヒープ
  - `BinaryHeap::append` / `extend`（`examples/binary_heap_extend.rs`）：std と同じ `rebuild_tail`（足す数が多ければ全体を作り直し、少なければ 1 つずつ sift_up。2048 要素を超えたら別の閾値）なので、足した後の並びも一致する
  - `Vec::extend`。引数は Vec・配列・`&v` / `&[..]`・範囲・イテレータ（IntoIterator の代わりに codegen が `into_iter()` / `iter()` を補う。要素の参照なら Copy として足す）
  - 比較の本体は型ごとに codegen が作る（`__exr_cmp::<T>`。Box / Vec を使わないプログラムではその場で展開する）
- Drop（`examples/drop.rs`、`examples/drop_more.rs`。drop の順序を rustc と比べ、valgrind でリーク・二重解放がないことを確かめる）：
  - `impl Drop for S`（ジェネリックな impl も）、`drop(x)`
  - スコープの終わりで宣言と逆順に drop する。対象は return / `?` / break / continue で抜けるスコープ、値を捨てる式文、`let _ = e`、代入で上書きされる古い値
  - ムーブを追跡する：値として読む・値で受ける `self`・`move` キャプチャ・値で束縛するパターン（`if let Some(b) = o`）・`for x in arr`
  - 一時値（`make().area()`、`&Box::new(..)`、match 対象）は文の終わりで drop する。let の初期化式の一時値は let の終わりで drop する。ただし初期化式がその構文で延ばすとき（`&temp()`。タプル・配列・キャスト・struct リテラル・ブロックの末尾の中のものも）と、束縛した値の型が参照を含むときは、ブロックの終わりまで残す。`if let` の match 対象の一時値は、Rust 2024 と同じく else のブロックの前で drop し、then のブロックの後でも drop する（その値が参照を含みうるときは文の終わりまで残す）
  - フィールドだけのムーブ（`self.top = n.next`）は、元のフィールドを 0 で埋めて持ち主を drop する
- 型：
  - 整数（`i8`〜`i64`、`u8`〜`u64`、`isize`、`usize`）、`bool`、`char`、`()`、`!`
  - `&T`、`&mut T`、`*const T`、`*mut T`
  - `[T; N]`（N はリテラルか const）、`&[T]`、`&mut [T]`、`&str`
  - タプル、struct、enum
  - `core::ffi::c_int` などの C 型の別名
- 式：
  - 算術・ビット演算・比較・短絡評価・`as`
    - キャストは整数 ⇔ char / bool、参照 → ポインタ、ポインタ ⇔ usize、フィールドのない enum → 整数
  - `let`（分解も）、代入、複合代入
    - 左辺はローカル変数・フィールド・`*p`・`a[i]`・static
  - `if` / `match` / `if let` / `while let` / `while` / `loop` / `for`（`break` / `continue` / `return`）
    - ラベル付きの `break 'outer` / `continue 'outer`、`break v`（`loop` の値）、ラベル付きブロック `'a: { .. break 'a v; .. }`
    - `let PAT = e else { return; };`
  - `for` で回せるもの：
    - `a..b`、`a..=b`、`(a..b).rev()`
    - 配列・スライス（`&xs`、`&mut xs`、値渡し）と、それに `.iter()` / `.iter_mut()` / `.copied()` / `.enumerate()` / `.rev()` を付けたもの
  - 構築：struct / タプル / enum / 配列（`[a, b]`、`[x; N]`）
  - 添字 `a[i]`（範囲外なら panic）、`&a[lo..hi]`（`..` / `lo..` / `..=hi` も）
  - `&[T; N]` → `&[T]` の暗黙の変換
  - `==` / `!=`：struct / enum / タプル / 配列は構造的に比べる。手書きの `eq` があればそれを呼ぶ
  - リテラル：`"..."`（`&'static str`）、`b"..."`、`b'x'`、`'x'`、負のリテラル
  - `?`（Option / Result）
  - マクロ：`panic!` / `unreachable!` / `todo!` / `assert!` / `assert_eq!` / `assert_ne!` / `debug_assert*!` / `matches!`
    - メッセージは書式化しない
    - `assert_eq!` / `assert_ne!` は `==` / `!=` として比べる（Option・Vec・String と `&str` なども）
    - マクロの引数の中のクロージャも使える（`println!("{:?}", v.iter().map(|x| x * 2).collect::<Vec<_>>())`、`examples/closures_in_macros.rs`）。
      クロージャは最初の `|` の位置で識別する（codegen がマクロの引数を parse し直してもノードの対応が取れる）
- パターン：
  - `_`、束縛（`ref` / `mut`）、`x @ p`、リテラル、範囲、`|`（`A(x) | B(x)` のように束縛してもよい）
  - スライスパターン `[a, b]` / `[first, .., last]` / `[x, rest @ ..]`（配列・スライス・`v[..]`）
  - バリアント、struct、タプル、`&p`、入れ子、ガード
  - 参照を match したときのデフォルト束縛モード
  - `None` などの unit バリアント
- メソッド：
  - 自動 deref と自動参照、UFCS（`S::m(&x)` / `Trait::m(&x)` / `<S as Trait>::m(&x)`）
  - 静的ディスパッチ。固有メソッド → trait 実装 → デフォルトメソッドの順に探す
- 浮動小数点 `f32` / `f64`：
  - 四則演算・`%`・比較・`as` キャスト（整数への変換は rustc と同じ飽和）、`1.5` / `1e-3` / `2.0f32` リテラル
  - メソッド：`sqrt` / `abs` / `floor` / `ceil` / `round` / `trunc` / `powi` / `powf` / `exp` / `ln` / `sin` / `cos` / `atan2` / `hypot` / `mul_add` / `max` / `min`（NaN を無視）/ `total_cmp` / `is_nan` / `to_bits` など（超越関数は libm）
  - 定数：`f64::MAX` / `EPSILON` / `NAN` / `INFINITY` / `consts::PI` など。`f64::sqrt(x)` のパス呼び出し
  - 書式化は rustc と同じ桁：最短表現（往復で同じ値になる最短の桁、`{:?}` は指数が -5 以下か 16 以上で `1e-7` 形式）、`{:.N}`（正確な 10 進展開から偶数丸め）。10 進変換は 1e9 進の多倍長で正確に行う
  - `"1.5".parse::<f64>()`（strtod、`ParseFloatError`）
  - `examples/float_fuzz.rs` の 6000 行（乱数のビット列の Display / Debug / `{:.N}` / parse）が rustc と一致する
- 128 bit 整数 `i128` / `u128`：
  - 算術・比較・シフト・ビット演算は Cranelift の I128（引数と戻り値は `enable_llvm_abi_extensions` で SysV と同じ 2 レジスタ）
  - 除算・剰余、浮動小数点との変換（丸めは 1 回）、書式化（`{}` / `{:x}` / `{:b}` / 幅など）、`parse` は prelude の関数
  - `checked_*` / `wrapping_*` / `pow` / `count_ones` / `MAX` / `MIN`、HashMap のキー、derive
  - `examples/int128.rs` の乱数による照合（3000 組の四則演算・シフト・比較）が rustc と一致する
- そのほか：
  - 整数の `overflowing_*` / `checked_pow` / `saturating_pow` / `rem_euclid` / `div_euclid` / `next_power_of_two` / `leading_ones` / `isqrt` / `from_str_radix`
  - `i32::pow(2, 10)` のような、プリミティブのメソッドを関数の形で呼ぶもの
  - `From` の impl と `.into()`（期待型の `from`。String は `String::from`、数値はキャスト）
  - `vec![v; n]` の要素が Copy でないもの（clone して並べる）、`Vec<Vec<T>>` の `concat`、`collect::<String>()` の要素が `&str` / `String`
  - `partition` / `rposition`、`collect::<Option<Vec<T>>>()` / `collect::<Result<Vec<T>, E>>()`、メソッドの turbofish の型推論
  - `String::from_utf8` / `std::str::from_utf8`（検査と `Utf8Error` の表示は std と同じ）、`trim_matches` 系（パターンは &str / char / クロージャ）、`split` / `lines` / `split_whitespace` の `rev()`
  - `Rc`（`clone` / `strong_count` / `ptr_eq`。弱参照はない）、`RefCell`（`borrow` / `borrow_mut` は参照を返すだけで、借用の検査はしない）、`Cell`
  - メソッドが見つからないときの `deref` 先での検索（`rc.borrow_mut()`）。`struct Node { children: Vec<Node> }` や `Vec<Rc<RefCell<Node>>>` のような再帰型
  - 書式の `{:e}` / `{:E}`（精度つきも）、幅・精度を引数から取る `{:w$}` / `{:.p$}` / `{:.*}`、`{:12e}` のような幅と型の組み合わせ
  - `println!` などの書式マクロの引数の中の型推論（parse し直したノードの型はソース上の位置で codegen に渡す）
  - 終わりのない範囲 `(1..)`、`char::is_ascii_digit` などを関数の値として渡すこと
  - `use E::*;` で取り込んだバリアントを短い名前で使うこと、`Box` を通した再帰型の derive(PartialEq)（比較の glue 関数を作る）
  - `try_into`（整数どうし。範囲外は `TryFromIntError`）、`i64::from(x)`、char の範囲 `'a'..='e'`
  - `BTreeMap::range`（`a..b` / `a..=b` / `a..`）、`HashMap` の `entry(..).or_default()`
  - `for x in (0..n).step_by(k)` のようにアダプタを通した for のループ変数の型推論
  - `Vec` の `rotate_left` / `rotate_right` / `split_at`、`Iterator::eq`、`std::cmp::Reverse(x)` のような `std::` 付きのパスでの構築
  - ローカル変数をキャプチャして返すクロージャ（`let mut c = 0; move || { c += 1; c }`。型は注釈かリテラル・String の初期化式から）
  - `?` でのエラーの `From` 変換（`impl From<ParseIntError> for AppError`）
  - 関数の中に書いたアイテム（`fn` / `struct` / `enum` / `const` / `use`。字句レベルでファイルの末尾に移す）
  - `std::collections::HashMap::new()` のような長いパス、`VecDeque::from(vec)`、スライスの `split_at` / `split_at_mut`、`Chars::as_str`
  - 標準入出力：`io::stdin()` の `read_line` / `lines()` / `read_to_string` / `lock()`、`io::stdout()` / `lock()` / `flush()`、`BufWriter::new(..)`、`write!` / `writeln!` を `write_str` を持つ任意の型に（libc の stdio を使うので `print!` と順序が混ざらない）
  - `&[u8]` と `Cursor` の `io::Read`、`&[u8]` の `io::BufRead`、任意の `Read` の上の `BufReader::new(r)` / `with_capacity`（`fill_buf` / `consume` / `read`。`B` が `BufRead` なら `&mut B` も）、`Cursor` の `io::Seek` / `SeekFrom`
  - `std::process::exit`、`dbg!`、`for i in 0..`（終わりのない範囲）、`char_indices()` / `bytes()` の `rev()`
  - クロージャを含む型を返すクロージャ（`flat_map(|i| (0..i).map(move |j| (i, j)))`、`|a| move |b| a + b`）: 型推論がクロージャの struct 型（親の型引数・キャプチャ・引数・戻り値）まで解く
  - `Iterator::cycle`、`Box<T>` の `as_ref()` / `as_mut()`
  - 書式化のビルダー `f.debug_struct(..)` / `debug_tuple` / `debug_list` / `debug_set` / `debug_map`、`f.alternate()` / `width()` / `precision()` / `pad()`
  - `HashMap::from([..])`（HashSet / BTreeMap / BTreeSet / VecDeque / BinaryHeap も）、HashMap の `retain` / `drain` / `extend`、HashSet / BTreeMap / BTreeSet / String の `extend`、値としての `[a, b].into_iter()`
  - `Ord::cmp(&a, &b)` / `PartialOrd::partial_cmp(..)` の UFCS
  - char のリテラル・範囲のパターン（`'a'..='z'`）、文字列リテラルのパターン（`match s { "add" => .. }`）
  - 引数のパターン（`fn index(&self, (i, j): (usize, usize))`）、参照への演算子の impl（`impl Add for &M`）
  - `Iterator` を受ける引数への `&Vec` / スライス / 配列（`a.iter().zip(&b)`）、範囲の添字を受け手にしたメソッド（`v[a..b].iter()`）
  - ユーザの `impl IntoIterator for &Coll`（`for x in &c`）、型パラメータ以外を縛る where 節（`where C::Item: Debug` / `where Self: Sized`。読み飛ばす）
  - `Box<dyn Error>`（`dyn Error + Send + Sync` も。変換した時点で `{}` / `{:?}` の文字列にした prelude の型で表す。`?` / `Box::new(e)` / `"msg".into()` で変換）、`impl std::error::Error for E {}`、`fn main() -> Result<(), E>`（Err なら `Error: {:?}` を stderr に出して終了コード 1）
  - 自分を `Option<Box<..>>` で含むジェネリックな型（`Node<T> { next: Option<Box<Node<T>>> }`）
  - ブランケット impl（`impl<T: Display> Describe for T`。境界は検査せず、メソッドが見つからないときにどの型でも探す）
  - `-> impl Iterator<Item = T>` などクロージャ以外の `impl Trait` の戻り値（プログラムを組み立てた直後に本体を型推論して具体型を入れる。呼び出し側のキャッシュのキーには解いた型が入る）
  - `const fn` を const の初期化式から呼ぶこと、範囲パターンの端に const（`i32::MIN..=-1`）、`?Sized` 境界
  - const ジェネリクス `const N: usize`（関数・impl・struct の型パラメータ）
    - 配列の長さ `[T; N]`、値としての `N`（`for i in 0..N`、`[0; N]`、`N * 10`）
    - 引数 `f::<3>()` / `Buf<4>` / `Mat::<C, R>::zero()` / const の名前 `zeros::<K>()`、引数の配列からの推論（`sum([1, 2, 3])`）
    - 型引数の位置には長さを持つ配列型 `[(); n]` を入れ、インスタンスの中の `N` はそこから読む（ジェネリックな型の長さは `CONST_LEN_BASE + i`）。式の長さ（`[T; N * 2]`）は未対応（rustc の安定版も同じ）
  - `Type::<A>::f()` の型のセグメントの型引数（`Vec::<i32>::new()` も）
  - 型の違う `==` / `!=` は trait で解決する：右辺の型を持つ `impl PartialEq<R> for S`（手書きのものも）を、両辺の型（片方の参照を外したものも）で選んで `eq` を呼ぶ
    - prelude に std と同じ組み合わせ：`Vec<T>` と `[U; N]` / `&[U; N]` / `&[U]` / `[U]` / `Vec<U>`、`[T]` と `Vec<U>` / `[U; N]`、`[T; N]` と `[U]` / `&[U]` / `[U; N]`（要素の型も違ってよい：`Vec<String> == ["a", "b"]`）
    - impl の self 型に配列・スライス（`impl .. for [T; N]` / `for [T]`）
    - `a[..2] == [1, 2]`（サイズ不定のスライスの比較は参照で比べる）
  - 型の違う `<` / `<=` / `>` / `>=` も同じく `impl PartialOrd<R> for S` の `partial_cmp` で（`Meters < Feet`、`Ver < (1, 5)`、`Ver < 2`）
    - 右辺がリテラル・タプル・配列なら、左の型の impl のうち形の合う R を期待型にする（`(1, 5)` を `(u32, u32)` に）。`==` も同じ
    - `partial_cmp` が `None` なら 4 つともすべて false（rustc と同じ。同じ型の手書きの partial_cmp も）
    - f64 / f32 の `partial_cmp` は NaN で `None`
- panic：
  - `#[panic_handler]` があればそれを呼ぶ（範囲外アクセス・`unwrap`・`assert!`・`panic!`・オーバーフロー検査など）
  - なければ trap する。std のプログラム（`no_std` でないもの）では main の最初に trap（SIGILL）のシグナルハンドラを登録し、
    rustc の panic と同じく stdout を書き出して終了コード 101 で終わる
  - stderr には rustc と同じ形で、場所と種類ごとのメッセージを出す（`thread 'main' panicked at src/main.rs:行:列:` の次の行。ファイル名は `src/main.rs` 固定）
    - オーバーフロー・0 除算・`MIN / -1`・シフト量、`panic!` / `unreachable!` / `todo!` / `unimplemented!`（書式つきも）、`assert!`（式の文字列か独自のメッセージ）、`assert_eq!` / `assert_ne!`（`left` / `right` の値を `{:?}` で。独自のメッセージも）
    - 範囲外の添字は `index out of bounds: the len is N but the index is I`
    - `Option` / `Result` の `unwrap` / `expect` は呼んだ側の場所（`#[track_caller]` 相当）。`Result::unwrap` の `Err` の値は、書式化を使うプログラムでだけ `{:?}` で出す
  - `assert_eq!(a, b)` は `let l = &(a); let r = &(b); if !(*l == *r) { panic }` に展開し、a と b を 1 回だけ評価する。
    右の型は左から決める（`assert_eq!(o, None)`）。左が数のリテラルなら右を先に評価して左の型を右から決める（`assert_eq!(2, n)`）。
    値を書式化するので、assert_eq! を使う std のプログラムには書式化の部品が付く
  - 場所の行はアイテムの開始行を足したファイルの中の行。場所を書き込んだ関数のキャッシュは開始行を覚えておき、上に行を足して行がずれたらコンパイルし直す
  - しくみ：trap ごとに別の trap コードを付け、codegen が「関数の中のオフセット → 種類・行・列・メッセージ」を記録する。
    driver がそれを static の表 `__EXR_TRAP_TABLE`（1 か所 8 byte）にし、SIGILL のハンドラ（`SA_SIGINFO`）が発生したアドレスで引く。
    検査そのものは `trapnz` のままなので、検査の速さは変わらない。範囲外の添字だけは長さと添字を渡すため、関数ごとに 1 つの冷たいブロックから `__exr_panic_index` を呼ぶ
- オーバーフロー検査（rustc の debug ビルドと同じ。`-O` なら入れない）：
  - `+` / `-` / `*`（と `+=` など）、単項の `-`、`abs`、`pow`、`<<` / `>>` の量が型の幅以上
  - 0 除算と `MIN / -1`（rustc と同じく `-O` でも検査する）。除数・シフト量が定数なら省く
  - `sum` / `product`（std と同じく呼び出し側の設定に従う: prelude の `#[rustc_inherit_overflow_checks]`）
  - prelude の中は検査しない（ハッシュなどで折り返す演算を書いている）。128 bit の掛け算は検査しない
  - 検査は分岐でなく条件つき trap（`trapnz`）にした。分岐とブロックで panic の場所を渡すと gen1000 のコンパイルが +50%、`trapnz` なら +5%

## core の代わり（組み込みの prelude、`crates/syntax/src/prelude_lib.rs.in`）

- `Option<T>`：`is_some` / `is_none` / `unwrap` / `unwrap_or` / `ok_or` / `or` / `xor` / `take` / `replace` / `as_ref` / `as_mut` / `copied` / `cloned`
  - クロージャを取るもの：`map` / `and_then` / `unwrap_or_else` / `map_or` / `is_some_and` / `filter` / `ok_or_else`
- `Result<T, E>`：`is_ok` / `is_err` / `unwrap` / `unwrap_err` / `unwrap_or` / `ok` / `err`
  - クロージャを取るもの：`map` / `map_err` / `and_then` / `unwrap_or_else`
- `core::panic::PanicInfo`
- `core::cmp::{min, max}`、`core::mem::{swap, replace, size_of, align_of}`、`core::ptr::{null, null_mut, read_volatile, write_volatile}`
- 組み込みメソッド：
  - 整数：`wrapping_*` / `checked_*`（→ Option）/ `saturating_*` / `min` / `max` / `abs` / `abs_diff` / `signum` / `pow` / `count_ones` / `leading_zeros` / `trailing_zeros` / `is_power_of_two` / `rotate_*` / `swap_bytes` / `is_ascii_*` / `to_ascii_*`
  - 関連定数：`u32::MAX` / `MIN` / `BITS`
  - 配列・スライス：`len` / `is_empty` / `get` / `get_mut` / `first` / `last` / `swap` / `contains` / `as_ptr` / `as_mut_ptr`
  - `&str`：`len` / `is_empty` / `as_bytes` / `as_ptr`
  - 生ポインタ：`add` / `sub` / `offset` / `is_null` / `read` / `write` / `read_volatile` / `write_volatile` / `cast`

prelude は、ユーザのコードの識別子から辿れるアイテムだけを付ける。`Option` を使わないプログラムには何も足さないので、キャッシュもビルド時間も増えない。

- `core::cmp::max` などは、ユーザが同じ名前の関数を定義してもぶつからないよう `__core_*` という名前で持っている。
- それらは、コードに `core::` か `use core::` が現れたときだけ付ける。

## 型推論と検査

- 型推論は関数単位の単一化で行う（`crates/sema/src/ast_infer.rs`）。
  - 参照・ポインタ・配列・スライス、ジェネリックな ADT の形も単一化する。
  - リテラル・構築・呼び出しの型も記録し、codegen で期待型が分からないときに使う（`Pair { a: 1 }.swap()` の `1` を、後ろの使用箇所から `i64` に決める）。
- 借用検査・可変性検査・ムーブ検査・match の網羅性検査はしない。
  - rustc を通るプログラムなら意味は変わらない前提で、struct は常にコピーしている。
  - どの arm にも合わなければ trap する。
  - オーバーフローは rustc の debug ビルドと同じく検査する（`-O` なら wrap する。上の「オーバーフロー検査」）。

## 未対応

- async: `async fn` / `async {}` / `.await` は受け付けるが、ただのコードとしてコンパイルする。`async fn` は呼ばれた時に本体を実行してその値を返す（future ではない）。そのため一度も poll されない future も実行され、async ブロックに対する `pin!` / `Future::poll` は動かない
- `Box<[T]>`、`Box::into_raw` などの Box の関数（`new` 以外）
- `String + &str` / `Vec` のスライスメソッドの一部（prelude に書いたもの以外。`&v` を `&[T]` にすれば組み込みのスライスメソッドは使える）
- イテレータの制限：`DoubleEndedIterator` / `ExactSizeIterator` の trait（`rev` は next_back を持つ prelude の型だけ）、`collect` は Vec / String / HashMap / HashSet / BTreeMap / `Option<Vec<T>>` / `Result<Vec<T>, E>` など prelude にあるもの
- 関連型の制限：trait の関連型は `Self::X` / `T::X` / `<T as Tr>::X` の形だけ。関連型に境界（`type Item: Copy;`）は付けられない
- 書式化の制限：`{:p}`、`format_args!`、`Formatter` の `fill()` / `align()`
- 比較の制限：`partial_cmp` は浮動小数点の NaN 以外では常に `Some`。BinaryHeap の `drain` / `retain` / `peek_mut`。一般の `IntoIterator` 境界はない（`extend` は codegen が引数をイテレータにする）
- HashMap の制限：キーに `impl Hash` を手書きすること
- Drop の制限
  - panic では drop しない（unwind しない）
  - `impl Drop` を持つ型のフィールドの一部だけをパターンでムーブすると、残りのフィールドはリークする（`let Pair { a, .. } = p;` の `b`）
  - クロージャの本体から `move` キャプチャした値をムーブすること（FnOnce として消費する使い方）はエラーにする
  - `Box` の中身のフィールドをムーブすると（`let n = boxed; n.val`）、Box の領域はリークする
  - 一時値の寿命は rustc の規則の近似（let の終わりで drop される一時値を借用する値を拒否しない: E0716）
- クロージャの制限
  - Fn / FnMut / FnOnce を区別しない（`FnMut` のクロージャを `&dyn Fn` にしてもエラーにならない）
  - ローカル変数をキャプチャするクロージャを `impl Fn` で返すとき、型が注釈やリテラルから分からない変数（`let v = Vec::new();` など）
- `dyn` の制限：`Self` やジェネリクスを使うメソッド（object safe でないもの）は dyn 経由で呼べない。`dyn A + B` は不可
- 1 ファイルの中のモジュールは平らにするので、別のモジュールに同じ名前のアイテムがあるとぶつかる（`mod x;` の別ファイルは `x.rs` / `x/mod.rs` を読む）
- 関数の中のアイテムは末尾に移すので、ほかの関数の中の同名のアイテムとぶつかる。そのエラーの行番号は移した先のもの
- `macro_rules!` の制限：衛生 (hygiene) はない（展開はテキストの置き換え）。`$x:expr` などは次の区切りまでのトークン列として取る近似
- ジェネリックな関数の `-> impl Trait` の戻り値
- 参照の Vec を `iter().flatten()` すること（`into_iter().flatten()` は可）、型パラメータのある trait（`From<T>` は名前だけ持ち、同じ型への `From` の impl は 1 つまで）
- `impl Trait for [T; N]` のような、プリミティブ以外の組み込み型への impl
