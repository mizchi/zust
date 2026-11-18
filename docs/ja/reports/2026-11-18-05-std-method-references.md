# std のメソッドの参照の差を詰める (2026-11-18)

日付: 2026-11-18

zustc のワークスペースで、よく使う std のメソッド 20 個の references を rust-analyzer と 1 件ずつ比べた
(`navigate.rs` の中の呼び出しから)。

| メソッド | 前 (zustc / rust-analyzer) | 今 |
|---|---|---|
| `Option::map` | 214 / 246 | 266 / 266 |
| `Iterator::map` | 282 / 342 | 349 / 349 |
| `collect` / `push` / `filter` / `starts_with` / `clone` / `is_empty` / `len` / `as_str` / `contains` / `get` / `insert` / `trim` / `find` / `first` / `extend` | 数〜十数件の差 | 全部同じ |
| `ToString::to_string` | 0 / 607 | 605 / 613 |
| `Option::unwrap` / `<[T]>::iter` | 341 / 346、812 / 822 | 345 / 346、831 / 832 |

直した主なもの:

- **名前の解決**
  - 受け手の型と Deref の先の両方にあるメソッドは受け手に近い方を選ぶ (`String::len` と `str::len`)。前は 2 つになって答えていなかった。
  - Deref の先は blanket impl の trait より先に見る (`RefMut<Option<T>>` の `as_ref` は Option のもの)。
  - trait のメソッドの宣言と、その blanket impl (`impl<T: Display> ToString for T`) の両方が候補なら宣言を選ぶ。
  - 同じ名前の const・関数がほかのファイルやクレートにもあるときは、同じファイル・同じクレートのものを選ぶ (`const PRELUDE`、テストの `fn run`)。
  - `use .. as PTy` の別名の enum のバリアントを、同じ名前の構造体 (`Path`) と取り違えていたのを直した。
- **型**
  - 入れ子のアダプタの要素 (`Peekable<Skip<Args>>`)。
  - 型の別名の受け手 (`type Cur = Option<..>`)。
  - `<str as ToOwned>::Owned` などの射影。
  - `get(i)` の `I::Output`。
  - タプル構造体のフィールド (`self.0`)。
  - `&mut T` を取る関数 (`mem::take`)。
  - 関数の名前を渡した `and_then(str_lit)`。
  - `Some(e)`。
  - 本体が `if` / `match` のクロージャ。
  - `&|t: &str| ..`。
  - `let x;` の後の代入。
  - 修飾つきの型 (`&std::cell::RefCell<..>`) を射影と取り違えて捨てていたのを直した。
- **パターン**
  - 前の腕の本体が括弧なしで `,` で終わる腕の束縛 (`"-O" => o.opt = true, s if ..`)。
  - ガードに `==` がある腕。
  - `matches!(e, P(g) if g..)` のガードの束縛。
- **ライブラリの索引**
  - 本文を捨てたライブラリでも分かるように、タプルのバリアントのフィールドの書き方を定義に残した (proc_macro2 の `TokenTree::Literal(l)`)。
  - `macro_rules!` の結果の型の手がかりも定義に残した (`json!` → `json_internal!` → `Value`)。

まだ違うもの:

- `&*ptr` の型。
- `usize::BITS` の関連 const。
- クロージャの引数の `e.to_string()` (`map_err(|e| ..)` の `io::Error`)。
- rust-analyzer が impl の側の `to_string` (string.rs の特殊化) に行く 1 か所。

`python3 bench/lsp_parity.py` の一致の平均は前回と同じか上がった:

| 対象 | zustc | rust-analyzer |
|---|---|---|
| synth | 99.7% | 99.1% |
| fixture | 97.3% | 95.6% |
| zustc | 96.3% | 95.5% |
| ide | 98.1% (+2.8 点) | 97.9% |

references の p50 は zustc で 4.2 ms、rust-analyzer では 24.1 ms。依存クレートの索引を足した分、メモリは 66 MB になった (rust-analyzer は 2.4 GB)。
