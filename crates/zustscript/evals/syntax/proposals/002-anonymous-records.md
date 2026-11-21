# 002: 構造的なレコード型と、TS 風の関数型

状態: 評価中 (r2)

## 動機

`corpus/components.zsc` より。

1. コンポーネントごとに props の型に名前を付ける必要がある(`TodoItemProps`、`TodoListProps`、`AppProps`)
2. 1 か所でしか使わない小さな戻り値(`Stats`)にも型の名前が要る
3. 関数型が `fn(i32) -> ()` で、TS の `(id: number) => void` と見た目が違う
   - 引数名も書けないので、`.d.ts` は `(a0: i32) => void` になる

## 提案

### A. レコード型は構造的(`type` は別名)

- 型の位置に `{ name: T, .. }` を書ける
  - 例: `props: { todo: Todo, onToggle: (id: i32) => () }`
- **`type Todo = { .. }` は、TS と同じく構造的な型の別名**
  - フィールドの名前の集合とそれぞれの型が同じなら、同じ型(順序は問わない)
  - `Todo` の値を、同じ形の無名の型を受ける関数に渡せる。逆も同じ
  - 名前は、エラーメッセージや hover での表示と、`@wasm` の Rust の struct 名に使う
- **再帰するレコード型だけは名前で区別する**(`type Node = { children: Array<Node> }`)
  - 無限に展開される型の上で、構造的な等しさを決めないため
- **幅の部分型はなし**: `{ a, b }` を `{ a }` に渡すことはできず、余分なフィールドもエラー
  - TS とは違う
  - ただし `.d.ts` の側は TS の構造的な型なので、TS から渡す分には TS の規則で通る
- **空のレコード型 `{}` は書けない**(`{}` は常にブロック)

### B. レコードの値

- **ブロックかレコードかは、構文だけで決まる**(先読み 2 トークン。型には依存しない)
  - `{` の次が `ident :`、`ident ,`、`...` のどれかならレコード
    - `ident ,` で始まる中身は、ブロックとしては元々エラーになる字面なので、既存のプログラムとは衝突しない
  - それ以外はブロック
  - `{ x }`(1 つだけの省略形)だけはブロックのまま
    - 期待する型がレコードの所に書かれたら、``did you mean the record `{ x: x }`?`` を出す
- **省略形**: `{ todo, onToggle }`、`{ total, done }`(全部が省略形でもよい)
- **更新は JS と同じ**: `{ ...t, done: !t.done }`
  - `...` は先頭に 1 つ。後に書いたフィールドが勝つ
  - 名前を書くレコードでも同じ: `Todo { ...t, done: x }`
  - Rust 式の末尾の `..t` は廃止する。書いたら、直し方を示すエラーにする
  - `...t` をフィールドの後ろに書いたらエラー(JS では後に書いた方が勝つので、意図と逆になりやすい)
- **ブロックの中のレコードの値**: `if c { { ...t, done: x } } else { t }` と波括弧を二重にする
  - ブロックの最初が `ident :` なら、専用のエラー「レコードの値は、もう 1 組の波括弧で囲む」を出す
- **リテラルの型**: 型が構造的なので、`let r = { id: 1, title: "a", done: false }; f(r)`(f は `Todo` を受ける)はそのまま通る

### C. TS 風の関数型(一本化)

- 型は `(id: i32) => ()` だけ
  - 引数名は省ける: `(i32) => ()`
  - 関数型を返す関数型は括弧で囲む: `(a: i32) => ((b: i32) => ())`
  - `(T)` の後に `=>` が続かなければ、ただの括弧
- 旧式の `fn(i32) -> ()` は、直し方つきのエラーにする
- **関数の宣言は `fn f(..) -> R` のまま**
  - 宣言は `->`、値のクロージャと関数型は `=>` という区別にする(Rust と TS のそれぞれの慣習に合わせる)
- 引数名は、`.d.ts` と、エディタの hover / シグネチャの表示に使う
  - 型の等しさには関わらない(`(id: i32) => ()` と `(n: i32) => ()` は同じ型)
  - 表示には、注釈から来た方の名前を使う

### D. 分割代入とコンポーネントの props

- 引数の位置にレコードのパターンを書ける
  - 例: `fn TodoItem({ todo, onToggle }: { todo: Todo, onToggle: (id: i32) => () }) -> Jsx`
  - 001 の `let { x, y } = p` と同じ規則
- JSX の props の検査は今までどおり(足りない prop、余分な prop、型の違いはエラー)
- **任意のフィールド** `title?: String` は、`Option` を標準にする提案(003)で入れる
  - `title: Option<String>` の略記になり、JSX では省略できる

### E. `@wasm` での無名のレコード

- フィールドの名前と型の集合から決まる名前の Rust の struct を作る(同じ形には同じ struct を 1 つ)
- `type` で名前を付けていれば、その名前を使う(`ZsTodo`)

## 変更前 / 変更後(corpus/components.zsc)

```zsc
// 変更後
extern "react" fn useState<T>(init: T) -> (T, (next: T) => ()) = "useState";

type Todo = { id: i32, title: String, done: bool }

pub fn TodoItem({ todo, onToggle }: { todo: Todo, onToggle: (id: i32) => () }) -> Jsx {
  <li className={if todo.done { "done" } else { "" }} onClick={() => onToggle(todo.id)}>
    {todo.title}
  </li>
}

pub fn TodoList({ todos, onToggle, title }: { todos: Array<Todo>, onToggle: (id: i32) => (), title: String }) -> Jsx {
  <section>
    <h2>{title}</h2>
    <ul>{todos |> .map((t) => <TodoItem key={t.id} todo={t} onToggle={onToggle} />)}</ul>
  </section>
}

fn toggle(todos: Array<Todo>, id: i32) -> Array<Todo> {
  todos |> .map((t) => if t.id == id { { ...t, done: !t.done } } else { t })
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
```

## 出力

JS は今と同じプレーンなオブジェクト。分割代入は JS の分割代入になる。

```js
export function TodoItem({ todo, onToggle }) { .. }
return { ...t, done: !t.done };
```

```ts
export interface Todo { readonly id: i32; readonly title: string; readonly done: boolean }
export declare function TodoItem(props: { readonly todo: Todo; readonly onToggle: (id: i32) => void }): $ReactElement;
export declare function stats(todos: readonly Todo[]): { readonly total: i32; readonly done: i32 };
```

## 実装の注意(implementer の r1 の指摘から)

- `Ty::Rec(フィールド)` を足す。`type` の名前は別名の表に置き、表示用に型に添える
- `Ty::Fn` に引数名を持たせる(単一化と等価性では無視する)
- レコード同士の単一化に失敗したら、差分(足りない / 余分 / 型が違うフィールド)を返して表示する
- 構文の判定のための先読みは、`try_arrow_params` と同じ保存と巻き戻しで足りる

## 代替案

1. **名前で区別し、リテラルの型だけ遅延させる**
   - リテラルは使われた先でどちらにもなれる
   - ただし変数を経由すると、名前付きと無名の型が違うまま残る。r1 の high の指摘が一部残るので選ばない
2. **無名のレコードの値に印を付ける**(`#{ .. }`): 二重の波括弧はなくなるが、TS から離れる。選ばない
3. **Rust 式の更新 `{ done: x, ..t }`**: JS の spread と逆の意味に読めるので、選ばない
4. **Rust 風の関数型 `fn(id: i32) -> ()`**: 宣言と矢印は揃うが、クロージャ `(x) => e` と揃わない。選ばない

## 未決事項

- `pub` な関数の無名の戻り値の型に、`.d.ts` で名前を付けるか(`export type stats_Result = ..`)
- `void` を `()` の別名として受け付けるか
- 総称的な無名のレコード `{ value: T }` と、構造的な等しさ(型引数を含めて比べる)
- TS 側から余分なプロパティを持つオブジェクトが渡されたとき、`{ ...t }` の更新でそのプロパティもコピーされる(JS の spread)。これを許容するか

## 改訂履歴

- r2(r1 の指摘と、決定事項から)
  - `type` を構造的な別名にした
  - ブロックとの区別を構文だけで決めるようにした(`ident ,` でもレコード)
  - 更新を JS 式 `{ ...t, x }` にし、関数型を TS 風に一本化した
  - 引数の位置の分割代入を足した
  - `@wasm` での無名のレコードの struct 名と、`{}` の扱いを決めた
  - `title?:` は 003 に移した
