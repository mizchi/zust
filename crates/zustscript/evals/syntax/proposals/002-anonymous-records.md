# 002: 構造的なレコード型と、TS 風の関数型

状態: 評価中 (r4)

## 動機

`corpus/components.zsc` より。

1. コンポーネントごとに props の型に名前を付ける必要がある(`TodoItemProps`、`TodoListProps`、`AppProps`)
2. 1 か所でしか使わない小さな戻り値(`Stats`)にも型の名前が要る
3. 関数型が `fn(i32) -> ()` で、TS の `(id: number) => void` と見た目が違う
   - 引数名も書けないので、`.d.ts` は `(a0: i32) => void` になる

この文書では、`{ … }` は「中身を省略した」という意味で使う。コードとしての `...` は、spread と、パターンの残り(001)だけ。

## 提案

### A. レコード型は構造的(`type` は別名)

- 型の位置に `{ name: T, … }` を書ける
  - 例: `props: { todo: Todo, onToggle: (id: i32) => () }`
- **`type Todo = { … }` は、TS と同じく構造的な型の別名**
  - フィールドの名前の集合とそれぞれの型が同じなら、同じ型(順序は問わない)
  - `Todo` の値を、同じ形の無名の型を受ける関数に渡せる。逆も同じ
  - 名前は、エラーメッセージや hover での表示と、`@wasm` の Rust の `type` 別名に使う(E)
- **`type` はレコード以外にも書ける**
  - 関数型: `type OnToggle = (id: i32) => ()`
  - タプル: `type Pair = (i32, String)`
- **名前で区別したい型は `@nominal type`**(r2 の決定)
  - `@nominal type Meters = { v: f64 }` と `@nominal type Celsius = { v: f64 }` は別の型
  - 書き方は `@wasm fn` と同じ、宣言の前に付ける印
  - 値は名前を書いて作る: `Meters { v: 1.0 }`。無名の `{ v: 1.0 }` は `Meters` に渡せない
  - 印のない `type` は、今までどおり構造的
  - r4 では、`@nominal` を付けられるのはレコードだけ
    - `@nominal type UserId = i32` のような数値の newtype は、別の提案で扱う
- **再帰する型**
  - 再帰とは、別名と型引数をたどって自分に戻ること(`type Node = { children: Array<Node> }`)
    - enum の定義を通る循環は数えない。enum はもともと名前で区別するため
  - 再帰する型は名前で区別する(無限に展開される型の上で、構造的な等しさを決めないため)
  - ただし単一化では、再帰する名前を 1 段だけ展開する。そのため、無名のリテラルを渡せる
    - `let n: Node = { children: [] };` は通る
- **幅の部分型はなし**: `{ a, b }` を `{ a }` に渡すことはできず、余分なフィールドもエラー
  - TS とは違う
  - 単一化に部分型はない。単一化のほかにある規則は、001 の絞り込みの情報だけ
  - ただし `.d.ts` の側は TS の構造的な型なので、TS から渡す分には TS の規則で通る
  - そのとき余分なプロパティは、JS の挙動のまま残る
    - JS と `@wasm` の差分テストは、型にあるフィールドだけを比べる
    - 入口で作り直すかは未決事項
- **空のレコード型 `{}` は書けない**(`{}` は常にブロック)
- **表示名**
  - 注釈から来た名前があれば、その名前(`t: Todo` なら `Todo`)
  - なければ、形そのもの(`{ total: i32, done: i32 }`)
  - 同じ形の別名は、hover の補足として出すだけ(`= Stats`)。表示名にはしない
    - 別名を 1 つ足しても、離れた所のエラーの表示が変わらないようにするため

### B. レコードの値

- **ブロックかレコードかは、構文だけで決まる**(先読み 2 トークン。型には依存しない)
  - `{` の次が `ident :`、`ident ,`、`...` のどれかならレコード
    - `ident ,` で始まる中身は、ブロックとしては元々エラーになる字面なので、既存のプログラムとは衝突しない
  - それ以外はブロック
  - 判定は 1 つの関数にまとめ、式の `{` と block() の先頭の両方がそれを呼ぶ
- **`{ x }`(中身が識別子 1 つだけのブロック)の警告**(r2 の決定 3、r4 で範囲を決めた)
  - 型によらず、構文だけで出す
  - 出すのは、式の位置のブロックだけ。primary の `{` から読むもの
    - `let` の右辺: `let r = { x };`
    - 引数、フィールドの値、配列の要素
    - `=>` の後(クロージャの本体): `(t) => { t }`
    - match の腕の本体
    - JSX の `{…}` の中に書いたブロック: `style={{ color }}`
      - JSX の `{text}` そのものはブロックではないので、対象外
    - ブロックの中の式として書いた `{ x }`(下の `only_total` の末尾)
  - block() が直接読む本体には出さない: fn / if / else / while / for の本体
    - `if x > a { x } else { a }`、`fn id(x: i32) -> i32 { x }`、`else { t }`、001 の `if found < 0 { None }` は警告なし
  - 直し方: レコードなら `{ x, }` か `{ x: x }`、値なら `x`
  - formatter は `{ x, }` の末尾のカンマを消さない
- **省略形**: `{ todo, onToggle }`、`{ total, done }`(全部が省略形でもよい)
- **更新は JS と同じ**: `{ ...t, done: !t.done }`
  - `...` は先頭に 1 つだけ。spread を 2 つ以上書くことはできない(未決事項)
  - 結果の型は `...t` の型と同じ
  - 後に書いたフィールドは、t にあるフィールドを同じ型で上書きするだけ
    - t にないフィールドを足す、フィールドの型を変える、はエラー
  - 後に書いたフィールドが勝つ(JS と同じ)
  - 名前を書くレコードでも同じ: `Todo { ...t, done: x }`
    - 001 のバリアントの更新 `Circle { ...c, r: 2.0 }` も、この規則に従う
  - Rust 式の末尾の `..t` は廃止する。書いたら、直し方を示すエラーにする
  - `...t` をフィールドの後ろに書いたらエラー(JS では後に書いた方が勝つので、意図と逆になりやすい)
- **ブロックの中のレコードの値**: `if c { { ...t, done: x } } else { t }` と波括弧を二重にする
  - block() の先頭で見るのは、レコードの 3 つの形(`ident :`、`ident ,`、`...`)の専用のエラーだけ
    - if や fn の本体の波括弧の直後に、これらの形が来たら「レコードの値は、もう 1 組の波括弧で囲む」
    - `{ x }` の警告は、block() の先頭では出さない
- **リテラルの型**: 型が構造的なので、`let r = { id: 1, title: "a", done: false }; f(r)`(f は `Todo` を受ける)はそのまま通る

### C. TS 風の関数型(一本化)

- 型は `(id: i32) => ()` だけ
  - 引数名は省ける: `(i32) => ()`
    - ただし `pub` と `extern` の signature の中の関数型で引数名を省くと、警告
    - TS では `(i32) => void` の `i32` が引数名に読めるため
  - 名前のある引数とない引数を混ぜるとエラー: `(a: i32, String) => ()`
    - 全部に名前を付けるか、全部省く
  - 型の `=>` は右結合。関数型を返す関数型の括弧は任意
    - `(a: i32) => (b: i32) => ()` は `(a: i32) => ((b: i32) => ())` と同じ
  - `(T)` の後に `=>` が続かなければ、ただの括弧
  - 1 要素のタプルの型は `(T,)`(値の 1 要素のタプルと同じ書き方)
- 旧式の `fn(i32) -> ()` は、直し方つきのエラーにする
- **関数の宣言は `fn f(…) -> R` のまま**
  - 宣言は `->`、値のクロージャと関数型は `=>` という区別にする(Rust と TS のそれぞれの慣習に合わせる)
  - 関数を返す関数: `fn adder(n: i32) -> (x: i32) => i32 { (x) => x + n }`
    - `->` の後が戻り値の型、型の中の `=>` は関数型、`{` からが本体
- 引数名は、`.d.ts` と、エディタの hover / シグネチャの表示に使う
  - 型の等しさには関わらない(`(id: i32) => ()` と `(n: i32) => ()` は同じ型)
  - 表示には、注釈から来た方の名前を使う

### D. 分割代入とコンポーネントの props

- 引数の位置にレコードのパターンを書ける
  - 例: `fn TodoItem({ todo, onToggle }: { todo: Todo, onToggle: (id: i32) => () }) -> Jsx`
  - 001 の `let { x, y } = p` と同じ規則
- **`let` と引数のレコードのパターンは、一部だけでよい**(r3 の決定 7)
  - TS の `const { x } = p` と同じ: `fn title_of({ title }: Todo) -> String { title }`
  - `...` は書いても書かなくてもよい
  - フィールドの書き漏らしがエラーになるのは、match のパターンだけ(001)
- `.d.ts` での引数名
  - コンポーネント(`Jsx` を返す関数)は `props`
  - それ以外は `arg0`、`arg1`(位置の番号)
- JSX の props の検査は今までどおり(足りない prop、余分な prop、型の違いはエラー)
- **任意のフィールド** `title?: String` は、`Option` を標準にする提案(003)で入れる
  - `title: Option<String>` の略記になり、JSX では省略できる

### E. `@wasm` での struct の名前

- struct は形(フィールドの名前と型の集合)ごとに 1 つ
- **struct 名は、モジュールの中の宣言順で決める**(ハッシュは使わない)
  - その形の別名がモジュールにあれば、最初に宣言した別名の名前(`type Todo` → `ZsTodo`)
  - なければフィールド名を並べた名前: `{ total: i32, done: i32 }` は `ZsRec_done_total`
  - 型の違う同じフィールド名の形が同じモジュールに複数あれば、モジュールの中で最初に現れた順に `_2`、`_3` を付ける
    - `{ total: f64, done: f64 }` が後に現れたら `ZsRec_done_total_2`
  - 名前はモジュールの名前空間に置く(`mod_a::ZsRec_done_total`)。決めるのに見るのは、そのモジュールの宣言と、それが import するモジュールだけ
    - 同じ形が import 先にもあれば、import 先の struct を使う(依存の順で先に決まる)
  - そのため、名前が変わるのは、同じファイルで別名を前に足したり、並べ替えたりしたときだけ。他のファイルの変更では変わらない
- **他の別名は、Rust の `type` 別名にする**: `type ZsTask = ZsTodo;`
- `@nominal` の型は、同じ形の別の型があっても、自分の struct(`ZsMeters`)を持つ

## 変更前 / 変更後(corpus/components.zsc)

```zsc
// 変更後
extern "react" fn useState<T>(init: T) -> (T, (next: T) => ()) = "useState";

type Todo = { id: i32, title: String, done: bool }
type OnToggle = (id: i32) => ()

pub fn TodoItem({ todo, onToggle }: { todo: Todo, onToggle: OnToggle }) -> Jsx {
  <li className={if todo.done { "done" } else { "" }} onClick={() => onToggle(todo.id)}>
    {todo.title}
  </li>
}

pub fn TodoList({ todos, onToggle, title }: { todos: Array<Todo>, onToggle: OnToggle, title: String }) -> Jsx {
  <section>
    <h2>{title}</h2>
    <ul>{todos |> .map((t) => <TodoItem key={t.id} todo={t} onToggle={onToggle} />)}</ul>
  </section>
}

fn toggle(todos: Array<Todo>, id: i32) -> Array<Todo> {
  todos |> .map((t) => if t.id == id { { ...t, done: !t.done } } else { t })   // 結果の型は t と同じ Todo。else { t } は警告なし
}

pub fn App({ initial }: { initial: Array<Todo> }) -> Jsx {
  let (todos, setTodos) = useState(initial);
  <TodoList title="todo" todos={todos} onToggle={(id) => setTodos(toggle(todos, id))} />
}

pub fn stats(todos: Array<Todo>) -> { total: i32, done: i32 } {
  let total = todos.len();
  let done = todos.filter((t) => t.done).len();
  { total, done }
}

fn title_of({ title }: Todo) -> String { title }   // 一部だけの分割代入(TS と同じ)。fn の本体なので警告なし

pub fn adder(n: i32) -> (x: i32) => i32 { (x) => x + n }

// 警告の出ない例(block() が直接読む本体)
fn larger(a: i32, x: i32) -> i32 { if x > a { x } else { a } }
fn id(x: i32) -> i32 { x }

// 書き誤りの例
fn only_total(todos: Array<Todo>) -> { total: i32 } {
  let total = todos.len();
  { total }   // warning: a block with only `total` is `total`; for a record write `{ total, }` or `{ total: total }`
}
fn pair_of(x: i32) -> () {
  let r = { x };   // warning: 同上(`let` の右辺)
}
type Bad = (a: i32, String) => ()   // error: name all the parameters of a function type, or none
```

## 出力

JS は今と同じプレーンなオブジェクト。分割代入した引数は、本体の先頭での展開になる(引数名は `.d.ts` と同じ)。

```js
export function TodoItem(props) { const { todo, onToggle } = props; … }
return { ...t, done: !t.done };
// @nominal type Meters = { v: f64 } の、TS から使うコンストラクタ(数値の brand と同じく、確かめるだけで変換しない)
export function Meters(v) { … return v; }
```

```ts
export type Todo = { readonly id: i32; readonly title: string; readonly done: boolean };
export type OnToggle = (id: i32) => void;
export declare function TodoItem(props: { readonly todo: Todo; readonly onToggle: OnToggle }): $ReactElement;
export declare function stats(todos: readonly Todo[]): { readonly total: i32; readonly done: i32 };
export declare function adder(n: i32): (x: i32) => i32;

// @nominal type Meters = { v: f64 }
export type Meters = { readonly v: f64 } & { readonly __zsc_nominal: "Meters" };
export declare function Meters(v: { readonly v: f64 }): Meters;
```

- `interface` ではなく `type` を出す(`type` は構造的な別名なので、TS でも同じ意味の書き方にする)
- `@nominal` の型は、数値と同じく brand を付ける。TS から `Celsius` や素の `{ v }` を渡すと、TS の型エラーになる
  - 値は `Meters({ v: 1.0 })` で作る

```rust
// @wasm(lower.rs)
struct ZsTodo { done: bool, id: i32, title: String }   // 別名 Todo がこのモジュールにあるので、その名前
struct ZsRec_done_total { done: i32, total: i32 }
struct ZsMeters { v: f64 }   // @nominal は自分の struct
```

## 実装の注意(implementer の指摘から)

- `Ty::Rec(フィールド)` を足す。`type` の名前は別名の表に置き、表示用に型に添える
- `@nominal` の型は、別名の表ではなく、名前を持つ型として登録する。レコード以外に付けたらエラー
- `Ty::Fn` の引数名は、等価性で無視する別のフィールドに持つ(`derive(PartialEq)` に混ざらないように)
  - ty() の `(` の中で、名前のある要素とない要素が混ざったらエラー
- 分割代入した引数は、内部では `$p0` という名前の引数にして、本体の先頭で展開する
  - jsx() の props の検査が `Ty::Rec` を受けるようにする
  - `let` と引数のレコードのパターンは、書き漏らしを検査しない。match のパターンだけが検査する
- レコード同士の単一化に失敗したら、差分(足りない / 余分 / 型が違うフィールド)を返して表示する
- 再帰する型は、別名と型引数の依存の強連結成分で見つける(enum の定義は辺にしない)
- checker はレコードの式の型を Id ごとに side table に記録し、lower はそれを引いて struct 名を決める
  - struct 名はモジュールごとに、宣言と最初に現れた順で決める(上の E)。プログラム全体は見ない
- 構文の判定のための先読みは、`try_arrow_params` と同じ保存と巻き戻しで足りる
- `{ x }` の警告は primary の `{` の側だけで出す。block() は 3 つの形の専用エラーだけを出す

## 代替案

1. **名前で区別し、リテラルの型だけ遅延させる**
   - リテラルは使われた先でどちらにもなれる
   - ただし変数を経由すると、名前付きと無名の型が違うまま残る。r1 の high の指摘が一部残るので選ばない
2. **無名のレコードの値に印を付ける**(`#{ … }`): 二重の波括弧はなくなるが、TS から離れる。選ばない
3. **Rust 式の更新 `{ done: x, ..t }`**: JS の spread と逆の意味に読めるので、選ばない
4. **Rust 風の関数型 `fn(id: i32) -> ()`**: 宣言と矢印は揃うが、クロージャ `(x) => e` と揃わない。選ばない
5. **struct 名をプログラム全体の最初に宣言した別名から取る**(r3)、**衝突したときだけ形のハッシュを付ける**(r4 の最初の案)
   - どちらも、どこか別のファイルに別名や同じフィールド名の形を足すと、既存の struct 名が変わる(非局所的)
   - 決める範囲をモジュールとその import 先に閉じ、宣言順で決めることにした
6. **`{ x }` の警告を、block() の本体にも出す**(r3)
   - `if x > a { x } else { a }` や `fn id(x) { x }` のような普通のコードに警告が出て、本当の誤りを見落とす
   - 廃止し、式の位置のブロックだけにした

## 未決事項

- `pub` な関数の無名の戻り値の型に、`.d.ts` で名前を付けるか(`export type stats_Result = …`)
- `void` を `()` の別名として受け付けるか(型の位置だけ)
- props の型とパターンで、フィールド名を 2 回書くこと(`{ todo, onToggle }: { todo: Todo, onToggle: … }`)を短くするか
- 総称的な無名のレコード `{ value: T }` と、構造的な等しさ(型引数を含めて比べる)
- spread を 2 つ以上書けるようにするか(`{ ...defaults, ...props }`。同じ型同士に限るか)
- TS から余分なプロパティを持つオブジェクトが渡されたとき
  - JS 層では残る。`{ ...t }` の更新でもコピーされる(JS の spread)
  - `@wasm` ではスロットに分解されて落ちる。同じソースで、JS と wasm の結果が変わりうる
  - 差分テストは型にあるフィールドだけを比べる(A)。今は JS の挙動のまま
  - 案: `pub` な関数の入口(TS から呼ばれる所)でだけ、型にあるフィールドを並べて作り直す
- 戻り値が関数型のとき、formatter が括弧を付けるか(`-> ((x: i32) => i32)`)

## 改訂履歴

- r4(r3 の指摘と、決定事項から。summary.md の表を参照)
  - `{ x }` の警告を、primary の `{` から読む式の位置のブロックだけにした。block() が直接読む fn / if / else / while / for の本体には出さない
  - block() の先頭で見るのは、3 つの形の専用エラーだけにした
  - `let` と引数のレコードのパターンは一部だけでよいことにした。書き漏らしのエラーは match だけ(決定 7)
  - struct 名を、モジュールの中の宣言順で決めるようにした(別名があればその名前、なければフィールド名、型の違う同名の形には `_2`)。決める範囲はモジュールとその import 先だけ。ハッシュは使わない
  - 表示名は注釈から来た名前だけにした。形からの逆引きは hover の補足
  - `.d.ts` を `interface` から `export type` にした
  - `@nominal` に `.d.ts` の brand とコンストラクタ `Meters({ v })` を足した。r4 ではレコードだけとした
  - 関数型で、名前のある引数とない引数の混在をエラーにした
  - 差分テストは型にあるフィールドだけを比べる、と書いた。入口で作り直す案は未決事項に
  - `fn adder(n: i32) -> (x: i32) => i32` の例と、警告の出ない例を足した
  - 部分型の書き方を 001 とそろえた(単一化に部分型はなく、ほかにある規則は絞り込みの情報だけ)
  - 未決事項に、複数の spread と、戻り値の関数型の括弧を足した。`void` は未決のまま
- r3(r2 の指摘と、決定事項から。summary.md の表を参照)
  - `{ x }` を、構文だけで常に警告にした。formatter は `{ x, }` のカンマを残す
  - `@nominal type` を足した。印のない `type` は構造的なまま
  - `type` で関数型とタプルにも名前を付けられるようにした
  - spread の規則を決めた(結果は `...t` の型、同じ型での上書きだけ、spread は 1 つ)
  - 再帰の定義を決め、単一化で 1 段だけ展開してリテラルを受けるようにした
  - 型の `=>` を右結合にし、1 要素のタプルの型を `(T,)` にした
  - `pub` / `extern` の関数型で引数名を省くと警告にした
  - ブロックかレコードかの判定を 1 関数にまとめ、block() の先頭でも専用のエラーを出すようにした
  - `@wasm` の struct 名と、表示名の規則を決めた
  - 分割代入した引数の `.d.ts` での名前(`props` / `arg0`)を決めた
  - パターンの残りは 001 の `...` にそろえた
- r2(r1 の指摘と、決定事項から)
  - `type` を構造的な別名にした
  - ブロックとの区別を構文だけで決めるようにした(`ident ,` でもレコード)
  - 更新を JS 式 `{ ...t, x }` にし、関数型を TS 風に一本化した
  - 引数の位置の分割代入を足した
  - `@wasm` での無名のレコードの struct 名と、`{}` の扱いを決めた
  - `title?:` は 003 に移した
