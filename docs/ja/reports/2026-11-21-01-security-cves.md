# Rust の CVE から作るセキュリティの回帰テスト、同じパターンの点検、wasm のサンドボックス

日付: 2026-11-21

zustc は安全に使えるか。それを確かめるため、この変更では Rust の過去のセキュリティの不具合 (標準ライブラリの CVE と
rustc の健全性の不具合) をテストのプログラムにし、zustc が同じ穴を通さないことを確かめ、見つけた穴ごとにその隣で同じ
誤りをしていないかを探した。あわせて wasm32-unknown-unknown の出力 (#7) を足し、モジュールがサンドボックスの中で動き、
そこから出ないことをテストした。テストは `tests/security/` と `tests/wasm/` にある (ケースの読み方はそれぞれの README)。

## 1. ケースと正解

ケースはそれぞれ、出典と安全なふるまいを書く。

- 出典: `//@ cve:` (std の CVE)、`//@ issue:` (rustc の健全性の不具合)、`//@ pattern:` (同じ実装のパターンの別の場所)、
  `//@ rule:` (それ自身の CVE はない言語の規則)
- ふるまい: 印を付けた行での拒否、panic、rustc のビルドと同じ出力の実行、`unsupported` (zustc にその API がないので、
  その CVE は起こりえない)

正解は rustc 1.97 で、そのビルドがヘッダと合っていることを確かめ、出力を比べる。実行するケースは 2 つのバックエンドで
ビルドし、valgrind の下で動かす。zustc がまだ通らないケースには `//@ todo: <理由>` の印を付ける。実行して報告するが
テストは失敗させず、通るようになったら印を外さなければならない。[TODO.md](../../../TODO.md) がそれをすべて並べる。

ケースは 100 個あり、それに加えて、どれも `unsafe` が要るはずの std の unsafe API 51 個の表がある。

## 2. std の CVE

`tests/security/cves.txt` は、RustSec の `std` パッケージの 18 件のアドバイザリと、Rust のブログの CVE-2024-24576 を、
それぞれのケースと並べる。表とケースが合っていることはテストが確かめる。CVE の番号・API・直った版は、アドバイザリの
ページで確かめた。

| 結果 | CVE |
|---|---|
| zustc に同じ不具合があった: 直した | CVE-2018-1000810 (`str::repeat`: `len * n` が wrap した。zustc のビルドはバッファの先へ書き続けた) |
| 通る | CVE-2015-20001、CVE-2017-20004、CVE-2018-1000657、CVE-2018-25008、CVE-2019-1010299、CVE-2020-36317、CVE-2020-36318、CVE-2020-36323 (普通の文字列)、CVE-2021-28877、CVE-2021-28879、CVE-2021-31162 |
| unsupported (API がないので不具合もない) | CVE-2020-36323 (プログラムの Borrow の impl)、CVE-2021-28875、CVE-2021-28876 / -28878、CVE-2021-29922、CVE-2022-21658 |
| 未対応 | CVE-2019-12083: impl が、trait の宣言していないメソッドを定義できる (E0407)。zustc には downcast がないので、型の取り違えにはならない |
| 当てはまらない | CVE-2024-24576 (Windows だけ) |

4 件は、確かめる前に API を足す必要があった: `VecDeque::reserve` / `capacity` / `make_contiguous`、`Arc::get_mut`、
`vec_deque::Iter` の Debug。どれも、その CVE の誤りが起こりえない書き方にした。wrap した deque は新しい領域へ順に写し、
イテレータはまだ残っている要素だけを出す。

zustc は unwind しないので、panic 安全性の CVE (CVE-2015-20001、CVE-2020-36317、CVE-2021-31162) は「panic がプログラムを
きれいに終わらせる」(exit 101、valgrind で問題なし) ことになる。それらが壊した状態が後で見られることはない。

## 3. 見つけた穴と、その隣の同じパターン

| パターン | 最初に見つけた所 | 隣で見つけたもの | 直したか |
|---|---|---|---|
| 桁あふれの検査なしで計算したサイズ (prelude は検査なしでコンパイルされる) | `str::repeat` | VecDeque の領域のサイズ、`HashMap::with_capacity` (`n * 4` が wrap し、`c * 3` の wrap で `c` が 0 になってループが終わらなかった) | すべて |
| 文字の途中のバイトでの String の編集 | `String::truncate` / `insert` / `insert_str` / `remove` / `replace_range` | スライス、`split_at`、スライスの `chars` (もとから検査あり) | すべて |
| E0133 を飛ばす下ろし方の経路 | クロージャの本体 | 生ポインタの `add` / `sub` / `offset` / `read*` / `write*`、`slice::from_raw_parts(_mut)`、`str::from_utf8_unchecked`、`get_unchecked(_mut)`、`mem::zeroed`、`Box::from_raw` (codegen が自分で下ろし、`check_unsafe_call` を通らない)。`MaybeUninit::assume_init*` と `hint::unreachable_unchecked` は prelude で safe な fn だった | すべて |
| marker trait 1 つだけの検査 | `unsafe` のない `impl Send` (E0200) | プログラム自身の `unsafe trait` の impl | すべて |
| 1 つの型だけで決めていた Send (`Rc` だけを探していた) | | 生ポインタ、Sync でない T の `Arc<T>` / `&T`、`MutexGuard`、Sync でない型の `static` (`static C: Cell<u32>`) | `dyn Trait` 以外のすべて (zustc の型は `+ Send` を保たない) |
| 長さを超える `rotate_left(k)` | | k を長さで割った余りにしていた | std と同じ panic にした |

std の unsafe API の表は、E0133 で拒否が 30、zustc にない (未知のものとして拒否) が 21 で、safe なコードで通るものは
ない。この変更の前は、上の経路が、生ポインタのメソッド・`from_raw_parts`・`from_utf8_unchecked`・`get_unchecked`・
`mem::zeroed`・`Box::from_raw`・`assume_init` を safe なコードで受け付けていた (`from_utf8_unchecked`・`get_unchecked`・
`Box::from_raw` はビルドで、ほかはコードで確かめた)。safe なコードが任意のアドレスを読み書きできた。

探したが見つからなかったもの:

- Vec・VecDeque・HashMap・String を変える間イテレータを持ち続けること (どれも拒否、E0502)
- `size_hint` や `ExactSizeIterator::len` が嘘を言うユーザのイテレータ、食い違う `Hash` / `Eq` / `Ord` の impl
  (collect・extend・HashMap・BinaryHeap・sort はメモリ安全なまま)
- VecDeque の添字と remove の経路 (境界を検査している)

まだ開いているもの (`//@ todo` の印付き、[TODO.md](../../../TODO.md) に一覧):

- privacy を検査しない
- RefCell に借用のフラグがない
- クロージャのキャプチャが貸し出しにならない
- `clear` の間 `&v[..]` を持ち続けられる
- E0716 と drop check
- unsafe な fn ポインタ
- E0407
- `dyn Trait` が `+ Send` を失う
- move して出した Box の領域が解放されない (`let`・`let` のパターン・`match` でのリーク。二重解放はない)

## 4. サンドボックスの中の wasm32-unknown-unknown

`zustc --target wasm32-unknown-unknown` は 1 つの wasm モジュールを書く。バックエンドは Cranelift のバックエンドと同じ
記録した命令を読むので、キャッシュとインクリメンタルな再ビルドに変更は要らない。examples/collections.rs の cold ビルドは
51 ms、関数 1 つの編集は 16 ms。

モジュールをサンドボックスに閉じ込めるもの:

- ホスト (`tests/wasm/run.mjs`) が与えるのは、fd 1 / 2、プログラムの引数、`exit`、純粋な関数だけ。ほかのもの
  (`fork` や `execvp` など) を import するモジュールは実行しない。
- モジュールの中:
  - 4 KiB の null ページ
  - 関数の入口ごとに検査するシャドウスタック
  - 2 GiB を上限とするメモリ
  - load と store ごとに 1 回の比較。null ページと、32 ビットを超えるアドレスで trap する (ポインタは 64 ビットのままで、
    縮めると別のメモリへ wrap するため)

`tests/wasm.rs` が確かめること:

- examples と `tests/security` の実行するケース (CVE のケースを含む) が rustc のビルドと同じものを出す: 65 個のプログラム
- プロセス・ホストのファイル・環境は拒否される
- でたらめなアクセス・偽の関数ポインタ・シグネチャの食い違い・スタックあふれ・巨大な確保は、trap か abort になる

examples・`tests/probe`・`bench/rt` では 135 個のプログラムが rustc と一致した。残りの 5 個は、サンドボックスが拒否する
もの (スレッド・プロセス・ファイル・stdin) を必要とする。`bench/rt` は Node の下で native の 3〜15 倍遅い。ブロックが
どれも 1 つの `loop` + `br_table` の場合分けになるためで、アドレスの検査はそのうち 5〜20% にあたる。

## 5. 残っていること

- 未対応のものの多くは、互換性と region check の作業が変えている型や借用の検査である。そちらに残し、理由付きで並べた。
- RefCell には `Ref` / `RefMut` のガードが要る。それには、ユーザの `Deref` の型を通した deref coercion が、より多くの所で
  働く必要がある。
- wasm:
  - ディスパッチャの代わりの relooper か stackifier
  - i128 の割り算
  - スレッド
  - zustc 自身による、wasm にした zustc のビルド
