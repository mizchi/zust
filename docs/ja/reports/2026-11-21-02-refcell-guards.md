# RefCell の借用フラグとガード、一時値を drop する時

日付: 2026-11-21

zustc の `RefCell` には借用フラグがなかった。`borrow` と `borrow_mut` はただの参照を返していた。そのため、1 つの
セルへの 2 つの `borrow_mut` が safe なコードに同じ値への `&mut` を 2 つ渡した。また、`borrow` を回すループの中の
`borrow_mut` が、ループの読んでいるバッファを解放しえた。この変更では `RefCell` に std と同じフラグを持たせ、`borrow` /
`borrow_mut` がガード `Ref` / `RefMut` を返すようにした。[セキュリティのレポート](2026-11-21-01-security-cves.md)が
残した [TODO.md](../../../TODO.md) の最初の項目である。

## 1. フラグとガード

prelude の `RefCell<T>` は、std のものと同じく `isize` を持つようになった。0 は空き、正の数はその数の共有の借用、-1 は
1 つの可変の借用を表す。

- `borrow` はセルが可変に借用されていれば panic し、`borrow_mut` は少しでも借用されていれば panic する。そうでなければ
  どちらもガードを返し、ガードの `Drop` が借用を返す。
- panic は std と同じもの (`RefCell already borrowed`、`RefCell already mutably borrowed`) を、std の
  `#[track_caller]` と同じく呼んだ所の行と列で出す。`unwrap` がすでに持つ仕組み (呼ぶ前に場所を書いておく) を使う。
- `try_borrow` と `try_borrow_mut` は `BorrowError` / `BorrowMutError` を返す。`Debug` と `Display` は std と同じ。
- `replace`・`replace_with`・`take`・`swap` は `borrow_mut` を通すので、std と同じく、生きている借用があれば panic する。
- 可変に借用されているセルの `{:?}` は `RefCell { value: <borrowed> }` を出す。
- `Ref::map`・`RefMut::map`・`Ref::clone` がある。
  - std と同じくメソッドではなく関連関数なので、`g.map(..)` と `g.clone()` は今までどおりガードを通して値のものになる。
  - パスは `Path::new` と同じく prelude の関数に対応させる。

## 2. ガードを通して値を使う

ガードが役に立つのは、その後ろの値を、直接借用したときのように使えるときだけである。std のコードがガードを使う形の
それぞれで、ガードを通す deref が別の場所に必要だった。

- `c.borrow().len`: 値のフィールド (型推論がガードの先を見る)
- `c.borrow().to_uppercase()`・`c.borrow_mut().push(1)`: 値のメソッド (`String` と `Vec` の組み込みのものを含む)
- `total` が `&[i32]` を受け取るときの `total(&c.borrow())`: `&Ref<Vec<T>>` から `&T`・`&[T]`・`&str` への型強制
- `*c.borrow()`: 一時値のガードの deref
- `for x in c.borrow().iter()`・`for x in g.iter_mut()`: ガードが持つものを回すループ

ここで rustc が拒否するものは拒否する。

- `Ref` を通した `iter_mut` や `&mut self` のメソッド (E0596)
- ガードを通した、`self` を値で受け取るメソッド (E0507)

## 3. 一時値を drop する時

ガードがあると、遅すぎる一時値の drop は正しいプログラムの panic になる。そのため、zustc が一時値を drop する時を
rustc に合わせる必要があった。合っていなかったのは 3 つである。

- `let` の初期化式の一時値は、すべてブロックの終わりまで残していた。rustc は、束縛した値がそれを借用していなければ
  `let` の終わりで drop する。zustc も今はそうする。束縛した値が借用しうるのは、その型が参照を含むときか、パターンに
  `ref` の束縛があるときである。以前は `let n = c.borrow().len();` がガードを生かしたままにし、次の `c.borrow_mut()` が
  panic した。
- 分解する `let` でも同じ: `let (n, _) = (c.borrow().len(), 0);`
- `if let` と `let ... else` の match 対象の一時値は、Rust 2024 と同じく else のブロックの前で drop する。以前は else の
  ブロックの中の `borrow_mut` が panic した。

1 つ目の修正で tests/ui の 2 つのテストが通るようになった: `nll/issue-53570.rs` と `mir/mir_early_return_scope.rs`。
`coercion/unsize-lifetime-shrinking.rs` は印の行で拒否されるようになったが、そのテストが確かめる理由とは別の理由なので、
`bench/rust_ui_accidental.txt` に載せた。

## 4. テスト

`tests/security/` に:

- `rt_refcell`: 1 つのセルの 2 つの借用を 3 通りに重ね、どれも panic しなければならない。`todo` の印を外した。
- `rt_refcell_iter_invalidation`: `c.borrow().iter()` を回すループが読んでいる間に、`borrow_mut` で Vec に push する。
  借用検査からは見えない (どちらの借用も共有の `&RefCell` のもの)。フラグが捕まえなければならない。
- `rt_refcell_ok`: 2 節の形でセルを使い、rustc のビルドと同じに動かなければならない。
- `rt_let_temporaries_drop`・`rt_scrutinee_temporaries_drop`: drop されると出力する値で、3 節の drop の時を確かめる。

drop の時を確かめている間に、以前からのリークが見つかった。値の一部をムーブし、残りを `_` か `..` に任せるパターンは、
その残りを drop しない: `let (a, _) = pair();`、`let W { a, .. } = w;`、`match pair() { (a, _) => .. }`。二重に drop
されるものはない。直すには、ムーブを追う処理の中で、パターンごとにムーブしない部分を drop することになる。そのため
`rt_pattern_partial_move` として残し、`todo` の印を付けて TODO.md に載せた。

## 5. 確認

- セキュリティのテストが通る (`cves.txt`、unsafe な API の表、TODO.md を含む)。
- wasm のテストが通る。RefCell のケースは wasm の下でも rustc のビルドと同じものを出す。
- tests/ui: 後退なし。
- examples・`tests/probe`・`bench/rt` の native のビルドが rustc のビルドと同じものを出す: 140 個のプログラム。
- 新しいケースと、この変更のために書いたテストのプログラムは、valgrind の下で問題なく動く。
