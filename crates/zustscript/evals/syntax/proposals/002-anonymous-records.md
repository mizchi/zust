# 002: 構造的なレコード型と、TS 風の関数型

状態: 評価中 (r5)

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
    - 任意のフィールド(`note?: T`、003)の印は比べない。`{ note?: T }` と `{ note: Option<T> }` は同じ型(003 の C)
  - `Todo` の値を、同じ形の無名の型を受ける関数に渡せる。逆も同じ
  - 名前は、エラーメッセージや hover での表示と、`@wasm` の Rust の `type` 別名に使う(E)
- **`type` はレコード以外にも書ける**
  - 関数型: `type OnToggle = (id: i32) => ()`
  - タプル: `type Pair = (i32, String)`
- **名前で区別したい型は `@nominal type`**(r2 の決定)
  - `@nominal type Meters = { v: f64 }` と `@nominal type Celsius = { v: f64 }` は別の型
  - 書き方は `@wasm fn` と同じ、宣言の前に付ける印
  - 値は名前を書いて作る: `Meters { v: 1.0 }`。無名の `{ v: 1.0 }` は `Meters` に渡せない
  - 逆向きも渡せない: `Meters` の値を、`{ v: f64 }` を受ける関数に渡すとエラー(r5)
    - 名前で区別する型は、自分の名前としか単一化しない
    - 直し方: フィールドを読んで作り直す(`{ v: m.v }`)
  - 別名は書ける: `type M2 = Meters` の `M2` は `Meters` と同じ型
  - TS から値を作るコンストラクタは、型と同名の namespace の中に出す: `Meters.make({ v })`(r5。出力を参照)
    - 同じ名前の `pub fn Meters` と衝突しない。001 の `.d.ts` の `Shape.Circle` と形がそろう
  - 印のない `type` は、今までどおり構造的
  - r5 でも、`@nominal` を付けられるのはレコードだけ
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
  - 表示名は型(Ty)には入れない。注釈と式の Id を鍵にした side table に置く(実装の注意)
    - 型に入れると、型の等しさに混ざり、単一化の順で残る名前が変わるため

### B. レコードの値

- **ブロックかレコードかは、構文だけで決まる**(先読み 2 トークン。型には依存しない)
  - `{` の次が `ident :`、`ident ,`、`...` のどれかならレコード
    - `ident ,` で始まる中身は、ブロックとしては元々エラーになる字面なので、既存のプログラムとは衝突しない
  - それ以外はブロック
  - 判定は 1 つの関数にまとめ、式の `{` と block() の先頭の両方がそれを呼ぶ
- **JSX の属性では、`{{` はレコード**(r4 の決定 10)
  - JSX の属性の `={` の直後の `{` は、中身によらず常にレコードとして読む
  - `style={{ color }}` は `{ color: color }`(React と同じ意味)
  - `style={{ color, fontSize: size }}`、`todo={{ ...t, done: true }}` も今までどおりレコード
  - 属性の値にブロックを書く理由はないので、失うものはない
  - 属性の `={{}}` は空のレコードになるので、エラー(空のレコード型は書けない)
  - JSX の子の `{…}`(`<p>{text}</p>`)は属性ではないので、この規則は使わない
- **`{ x }`(中身が識別子 1 つだけのブロック)の警告**(r2 の決定 3、r4 で範囲を決めた)
  - 型によらず、構文だけで出す
  - 出すのは、式の位置のブロックだけ。primary の `{` から読むもの
    - `let` の右辺: `let r = { x };`
    - 引数、フィールドの値、配列の要素
    - `=>` の後(クロージャの本体): `(t) => { t }`
    - match の腕の本体
    - JSX の子の `{…}` の中に書いたブロック: `<p>{{ x }}</p>`
      - JSX の属性の `={{ x }}` は、上の規則でレコードなので、対象外(r5)
      - JSX の `{text}` そのものはブロックではないので、対象外
    - ブロックの中の式として書いた `{ x }`(下の `only_total` の末尾)
  - block() が直接読む本体には出さない: fn / if / else / while / for の本体
    - `if x > a { x } else { a }`、`fn id(x: i32) -> i32 { x }`、`else { t }`、001 の `if found < 0 { None }` は警告なし
  - 直し方: レコードなら `{ x, }` か `{ x: x }`、値なら `x`
  - formatter は `{ x, }` の末尾のカンマを消さない
- **省略形**: `{ todo, onToggle }`、`{ total, done }`(全部が省略形でもよい)
- **更新は JS と同じ**: `{ ...t, done: !t.done }`
  - `...` は先頭に 1 つだけ。spread を 2 つ以上書くことはできない(未決事項)
    - それまでは、2 つ目の元のフィールドを並べて書く: `{ ...defaults, color: props.color, size: props.size }`
    - `{ ...defaults, ...props }` は、直し方(フィールドを並べる)つきのエラー
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
    - **formatter は、戻り値が関数型なら括弧を付ける**(r5): `fn adder(n: i32) -> ((x: i32) => i32) { (x) => x + n }`
      - 括弧がなくても同じ意味(構文としては任意)
      - `-> (x: i32) => i32 {` は、TS の人には本体の始まりやクロージャにも読めるため
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
- **推奨の形**(r5): props が 2〜3 個を超えるなら、型に名前を付けて、パターンでは名前だけを書く
  - `type TodoListProps = { todos: Array<Todo>, onToggle: OnToggle, title: String }`
  - `pub fn TodoList({ todos, onToggle, title }: TodoListProps) -> Jsx`
  - フィールドの型を 2 回書かずに済む。`type` は構造的な別名なので、`.d.ts` にも `TodoListProps` が出る
  - 小さな props は、今までどおり無名の型をその場に書いてよい(`TodoItem`)
- JSX の props の検査は今までどおり(足りない prop、余分な prop、型の違いはエラー)
- **任意のフィールド** `title?: String` は、003 で決める(003 の C を参照)
  - `title?: String` は `title: Option<String>` と**同じ型**。違いは、省略できるという印だけ
    - 印は型の一部ではない。A の構造的な等しさでも比べない
    - zsc の中では、どちらも Option の表現で持つ
  - 読むときの型は `Option<String>`
  - 印のあるフィールドは、リテラルと JSX の属性で省略できる。省略したら `None`
    - 注釈のないリテラルの省略は、003 の候補つきの型変数で決める
  - 印のあるフィールドのリテラルの値と JSX の属性の値には、`String` の値をそのまま書ける(Some として受ける。003)
  - TS との境界(`pub` の入口と出口、コンポーネントの props)では、`None` はキーなし、`Some(v)` は `v`(下の「出力」)

### E. `@wasm` での struct の名前

- struct は形(フィールドの名前と型の集合)ごとに 1 つ
- **struct はプログラム全体で、形ごとに 1 つ**(モジュールごとには作らない。構造的に同じ型は、Rust でも同じ型)
- **struct 名は、形だけから作る**: フィールドの名前と型を、名前の順に並べる
  - 宣言の順にも、他の項目やファイルにも依存しない。形を足しても、既存の struct 名は変わらない
- **名前の作り方は単射にする**(r5。違う形が同じ名前にならない)
  - r4 の `ZsRec_a_i32_b_i32` は、`{ a: i32, b: i32 }` と `{ a_i32_b: i32 }` が同じ名前になった。区切りの `_` がフィールド名にも入るため
  - フィールド名の前に、その長さを付ける: `4done`、`5total`
  - フィールドの型
    - 数値と bool は、そのまま(`i32`、`f64`、`bool` …)
    - 入れ子のレコードは、その形を印 `R` と `E` で囲む(別名の名前ではない。別名を足しても変わらないように)
    - enum と `@nominal` は名前で区別される型なので、**定義したモジュールで修飾した名前**を入れる
      - モジュールのパスの各部分と型の名前に長さを付け、印 `M` と `E` で囲む: `a/state.zsc` の `State` は `M_1a_5state_5State_E`
      - 別のモジュールの同じ名前の型(`a.State` と `b.State`)は別の名前になる。同じ型を指す別名(`type S = a.State`)は、定義の側の名前になるので同じ
      - モジュールがまだない今は、ファイルのパス(crate の根からの相対)を使う
  - 全体を `__zs` の名前空間に置く: `__zs_Rec_…`
    - 利用者の型名からは `__zs` で始まる名前は作れないので、衝突しない(`type Rec_x` の別名は `ZsRec_x` になる)
  - 例
    - `{ total: i32, done: i32 }` は `__zs_Rec_4done_i32_5total_i32`
    - `{ total: f64, done: f64 }` は `__zs_Rec_4done_f64_5total_f64`
    - `{ a: i32, b: i32 }` は `__zs_Rec_1a_i32_1b_i32`、`{ a_i32_b: i32 }` は `__zs_Rec_7a_i32_b_i32`
    - `{ p: { x: f64 }, y: f64 }` は `__zs_Rec_1p_R_1x_f64_E_1y_f64`、`{ p: { x: f64, y: f64 } }` は `__zs_Rec_1p_R_1x_f64_1y_f64_E`
  - 型引数のある enum(`Option<f64>`)は、名前の後に型引数を印 `A` と `E` で囲む: `6Option_A_f64_E`
  - **任意のフィールドの印は、名前に入れない**(003。印は型の一部ではないため)
    - `{ x?: f64 }` と `{ x: Option<f64> }` は、同じ struct `__zs_Rec_1x_6Option_A_f64_E`
  - 長くなるのは受け入れる(読むときは別名の `type` を見る)
  - 形から名前への写像が単射であることは、lower のテストで確かめる
  - `#[allow(non_camel_case_types)]` と `#[derive(Clone, Copy, PartialEq)]` を付ける
- **読める名前は、別名ごとの Rust の `type` 別名で出す**
  - `type Stats = { total: i32, done: i32 }` は `pub type ZsStats = __zs_Rec_4done_i32_5total_i32;`
  - `type Pt = { x: f64, y: f64 }`(モジュール a)は `pub type ZsPt = __zs_Rec_1x_f64_1y_f64;`(a の名前空間)
  - 別のモジュールの `type Point` が同じ形なら、`pub type ZsPoint = __zs_Rec_1x_f64_1y_f64;`。同じ struct なので、値をそのまま渡せる
  - 総称的な別名(`type Box<T> = { value: T }`)は、r5 では Rust の別名を出さない
    - lower は型引数ごとに単相化するので、出るのは単相化した形の名前(`__zs_Rec_5value_f64`)だけ
- **`@wasm` で使えるのは、今の部分集合と同じく Copy にできる形だけ**(数値、bool、それらのレコード)
  - 文字列や配列を含む形は、エラー: ``error: `Todo` has a `String` field, which an @wasm fn cannot take (records there must be Copy)``
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

type TodoListProps = { todos: Array<Todo>, onToggle: OnToggle, title: String }

pub fn TodoList({ todos, onToggle, title }: TodoListProps) -> Jsx {   // 推奨の形: 型は名前で、パターンはフィールド名だけ
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

pub fn adder(n: i32) -> ((x: i32) => i32) { (x) => x + n }   // formatter は戻り値の関数型に括弧を付ける

type Style = { color: String, size: i32 }

pub fn Badge({ color, text }: { color: String, text: String }) -> Jsx {
  <span style={{ color }}>{text}</span>   // 属性の `={` の直後なので、{ color: color } のレコード(警告なし)
}

fn themed(defaults: Style, props: { color: String }) -> Style {
  let s = { ...defaults, color: props.color };   // spread は 1 つだけ。2 つ目の元(props)は、フィールドを並べて書く
  s
}

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
// <span style={{ color }}> は、属性の中のレコード
_jsx("span", { style: { color: color }, children: text });
// @nominal type Meters = { v: f64 } の、TS から使うコンストラクタ(数値の brand と同じく、確かめるだけで変換しない)
export const Meters = { make(v) { … return v; } };
```

- 同じ名前の `pub fn Meters` もあるときは、関数に `make` を足す: `export function Meters(…) { … }` と `Meters.make = function (v) { … };`

```ts
export type Todo = { readonly id: i32; readonly title: string; readonly done: boolean };
export type OnToggle = (id: i32) => void;
export type TodoListProps = { readonly todos: readonly Todo[]; readonly onToggle: OnToggle; readonly title: string };
export declare function TodoItem(props: { readonly todo: Todo; readonly onToggle: OnToggle }): $ReactElement;
export declare function TodoList(props: TodoListProps): $ReactElement;
export declare function stats(todos: readonly Todo[]): { readonly total: i32; readonly done: i32 };
export declare function adder(n: i32): (x: i32) => i32;

// @nominal type Meters = { v: f64 }
export type Meters = { readonly v: f64 } & { readonly __zsc_nominal: "Meters" };
export declare namespace Meters { function make(v: { readonly v: f64 }): Meters; }
```

- 任意のフィールドを持つ型(003)
  - zsc の中のオブジェクトは、省略したフィールドにも `None` を持つ
  - TS へ渡すとき(`pub` の戻り値、JSX の props)は、`None` のキーを書かない。`Some(v)` は `v` を書く
  - TS から受けるとき(`pub` の引数、コンポーネントの props)は、キーが無い、`undefined`、`null` を `None` にする
  - そのため、この型の値は境界で作り直す。`.d.ts` は `readonly note?: string | undefined`
- `interface` ではなく `type` を出す(`type` は構造的な別名なので、TS でも同じ意味の書き方にする)
- `@nominal` の型は、数値と同じく brand を付ける。TS から `Celsius` や素の `{ v }` を渡すと、TS の型エラーになる
  - 値は `Meters.make({ v: 1.0 })` で作る
  - コンストラクタは型と同名の namespace の中にあるので、同じ名前の `pub fn Meters` と衝突しない(TS では関数と namespace が 1 つにまとまる)
  - 001 の `Shape.Circle`(型と同名の namespace)と形がそろう

```rust
// @wasm(lower.rs)
// Todo は String を含むので @wasm では使えない(エラー)
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq)]
struct __zs_Rec_4done_i32_5total_i32 { done: i32, total: i32 }   // stats の戻り値
pub type ZsStats = __zs_Rec_4done_i32_5total_i32;                // `type Stats = { … }` があれば
struct ZsMeters { v: f64 }   // @nominal は自分の struct
```

## 実装の注意(implementer の指摘から)

- `Ty::Rec(フィールド)` を足す。`type` の名前は別名の表に置く
- **表示名は Ty に入れない**(r5)
  - Ty は `derive(PartialEq)` で、unify_inner の最後も `a == b` で比べる。表示名を入れると等しさに混ざる
  - 型変数は先に単一化した側に置き換わるので、Ty に入れると、残る名前が単一化の順で変わる
  - 表示名は、注釈から来た束縛と式の Id を鍵にした side table に置く。エラーと hover はそれを引く
- `@nominal` の型は、別名の表ではなく、名前を持つ型として登録する。レコード以外に付けたらエラー
  - 単一化では、自分の名前としか等しくならない(同じ形の構造的な型とも等しくない)
  - `type M2 = Meters` は別名の表に入り、`Meters` に展開される
  - TS 向けのコンストラクタは namespace の `make` に出す。そのため、同じ名前の `pub fn` と JS の export が衝突しない
- `Ty::Fn` の引数名は、等価性で無視する別のフィールドに持つ(`derive(PartialEq)` に混ざらないように)
- `Ty::Rec` のフィールドの任意の印(003)も同じく、等価性で無視する別のフィールドに持つ。struct 名にも入れない
  - ty() の `(` の中で、名前のある要素とない要素が混ざったらエラー
- 分割代入した引数は、内部では `__zs_p0` という名前の引数にして、本体の先頭で展開する
  - r4 の `$p0` は Rust の識別子にならないので、lower.rs が予約している `__zs` の名前空間にした
  - jsx() の props の検査が `Ty::Rec` を受けるようにする
  - `let` と引数のレコードのパターンは、書き漏らしを検査しない。match のパターンだけが検査する
- レコード同士の単一化に失敗したら、差分(足りない / 余分 / 型が違うフィールド)を返して表示する
- 再帰する型は、別名と型引数の依存の強連結成分で見つける(enum の定義は辺にしない)
- checker はレコードの式の型を Id ごとに side table に記録し、lower はそれを引いて struct 名を決める
  - struct 名は形だけから作る(上の E)。形の表はプログラム全体で 1 つだが、名前を決めるのに他の項目を見る必要はない
  - lower.rs の `rs_type`(`Zs{n}`)を、形からの名前(`__zs_Rec_…`)と別名の `type` に変える
  - 形から名前への写像が単射であることを、lower のテストで確かめる(r4 の衝突の例、入れ子の例、`_` を含むフィールド名、enum の名前を含む形)
  - 総称的な別名には、Rust の `type` を出さない
- 構文の判定のための先読みは、`try_arrow_params` と同じ保存と巻き戻しで足りる
- `{ x }` の警告は primary の `{` の側だけで出す。block() は 3 つの形の専用エラーだけを出す
- JSX の属性を読む所で、`={` の直後が `{` なら、判定の関数を通さずにレコードとして読む(決定 10)

## 代替案

1. **名前で区別し、リテラルの型だけ遅延させる**
   - リテラルは使われた先でどちらにもなれる
   - ただし変数を経由すると、名前付きと無名の型が違うまま残る。r1 の high の指摘が一部残るので選ばない
2. **無名のレコードの値に印を付ける**(`#{ … }`): 二重の波括弧はなくなるが、TS から離れる。選ばない
3. **Rust 式の更新 `{ done: x, ..t }`**: JS の spread と逆の意味に読めるので、選ばない
4. **Rust 風の関数型 `fn(id: i32) -> ()`**: 宣言と矢印は揃うが、クロージャ `(x) => e` と揃わない。選ばない
5. **struct 名をプログラム全体の最初に宣言した別名から取る**(r3)、**衝突したときだけ形のハッシュを付ける**(r4 の最初の案)
   - どちらも、どこか別のファイルに別名や同じフィールド名の形を足すと、既存の struct 名が変わる(非局所的)
   - モジュールの中の宣言順で決める案(r4 の途中の案)も、同じファイルに形を足すと離れた項目の名前が変わり、互いを import しないモジュールの同じ形が Rust で別の型になる
   - 名前を形(フィールドの名前と型)だけから作り、読める名前は別名の `type` で出すことにした
6. **`{ x }` の警告を、block() の本体にも出す**(r3)
   - `if x > a { x } else { a }` や `fn id(x) { x }` のような普通のコードに警告が出て、本当の誤りを見落とす
   - 廃止し、式の位置のブロックだけにした
7. **struct 名の区切りを `_` だけにする**(r4)
   - `{ a: i32, b: i32 }` と `{ a_i32_b: i32 }`、入れ子の形どうしが、同じ名前になる(単射でない)
   - 2 つ目の形が 1 つ目の struct を使ってしまい、誤りが rustc のエラーとして離れた所に出る
   - フィールド名に長さを付け、入れ子を印で囲むことにした
8. **JSX の属性の `{{ x }}` もブロックとして読み、警告だけ出す**(r4)
   - React では `style={{ color }}` はオブジェクト。小文字の要素の属性には期待する型がないので、型エラーにならず、文字列が渡る
   - 属性ではレコードとして読むことにした(r4 の決定 10)

## 未決事項

- `pub` な関数の無名の戻り値の型に、`.d.ts` で名前を付けるか(`export type stats_Result = …`)
- `void` を `()` の別名として受け付けるか(型の位置だけ)
- 総称的な無名のレコード `{ value: T }` と、構造的な等しさ(型引数を含めて比べる)
  - 総称的な別名 `type Box<T>` に、Rust の総称の別名を出すか(r5 では出さない)
- spread を 2 つ以上書けるようにするか(`{ ...defaults, ...props }`。同じ型同士に限るか)
  - それまでの書き方は B(`{ ...defaults, color: props.color }`)
- レコードの値の `==`(構造的な等しさ。001 の enum の `==` と一緒に決める)
  - `@wasm` の `derive(PartialEq)` と、JS での比べ方の結果を一致させる必要がある
- TS から余分なプロパティを持つオブジェクトが渡されたとき
  - JS 層では残る。`{ ...t }` の更新でもコピーされる(JS の spread)
  - `@wasm` ではスロットに分解されて落ちる。同じソースで、JS と wasm の結果が変わりうる
  - 差分テストは型にあるフィールドだけを比べる(A)。今は JS の挙動のまま
  - 案: `pub` な関数の入口(TS から呼ばれる所)でだけ、型にあるフィールドを並べて作り直す
  - 任意のフィールドを持つ型は、003 で境界で作り直すことになった。その型では余分なプロパティが落ちる

## 改訂履歴

- r5(r4 の指摘と、決定事項から。summary.md の表を参照)
  - struct 名の作り方を単射にした。フィールド名に長さを付け、入れ子の形は `R` と `E` で囲み、enum と `@nominal` の名前にも長さを付ける。全体を `__zs` の名前空間に置く(`__zs_Rec_4done_i32_5total_i32`)。lower のテストで単射を確かめる
  - 別名は `pub type ZsStats = __zs_Rec_…;`。総称的な別名には、r5 では Rust の別名を出さない
  - 分割代入した引数の内部の名前を `$p0` から `__zs_p0` にした
  - 表示名を Ty に入れず、注釈と式の Id の side table に置くことにした
  - JSX の属性の `={` の直後の `{` は、常にレコードにした(決定 10)。`{ x }` の警告の範囲から属性を外し、`Badge` の例と出力を足した
  - `@nominal` のコンストラクタを、型と同名の namespace の `Meters.make({ v })` にした。`.d.ts` と JS の出力を直した
  - `@nominal` の値は同じ形の構造的な型に渡せない、`type M2 = Meters` は書ける、と書いた
  - props の推奨の形(`type TodoListProps = { … }` と `fn TodoList({ … }: TodoListProps)`)を載せ、`TodoList` の例を直した
  - formatter は戻り値の関数型に括弧を付けることにした(`-> ((x: i32) => i32) {`)。`adder` の例を直した
  - spread が 1 つの間の書き方(`{ ...defaults, color: props.color }`)を書き、`themed` の例を足した
  - 任意のフィールドは 003 に従うと書き直した。`title?: T` は `title: Option<T>` と同じ型で、違いは省略できる印だけ。構造的な等しさと struct 名に印は入らない。TS との境界では、`None` のキーを書かない(003 r1 の決定 1 に合わせた)
  - struct 名に、型引数のある enum の書き方(`6Option_A_f64_E`)を足した(003 r1 の決定 1 に合わせた)
  - 代替案に、`_` だけの struct 名と、JSX の属性の `{{ x }}` をブロックとして読む案を足した
  - 未決事項: props の 2 回書きと formatter の括弧を外した。レコードの `==` を足した(001 の enum の `==` と一緒に)

- r4(r3 の指摘と、決定事項から。summary.md の表を参照)
  - `{ x }` の警告を、primary の `{` から読む式の位置のブロックだけにした。block() が直接読む fn / if / else / while / for の本体には出さない
  - block() の先頭で見るのは、3 つの形の専用エラーだけにした
  - `let` と引数のレコードのパターンは一部だけでよいことにした。書き漏らしのエラーは match だけ(決定 7)
  - struct はプログラム全体で形ごとに 1 つにし、名前はフィールドの名前と型だけから作るようにした(`ZsRec_done_i32_total_i32`)。読める名前は別名ごとの `type` で出す。順序にもハッシュにも依存しない
  - `@wasm` で使える形は Copy にできるものだけと明記した
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
