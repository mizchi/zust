# 002: 無名のレコード型と、TS 風の関数型

状態: 草案

## 動機

`corpus/components.zsc` より。

1. コンポーネントごとに props の型に名前を付ける必要がある(`TodoItemProps`、`TodoListProps`、`AppProps`)。props は、そのコンポーネントでしか使わないことが多い
2. 1 か所でしか使わない小さな戻り値(`Stats`)にも型の名前が要る
3. 関数型が `fn(i32) -> ()` で、TS の `(id: number) => void` と見た目が違う。引数の名前も書けないので、`.d.ts` は `(a0: i32) => void` になる

## 提案

### A. 無名のレコード型(構造的)

- 型の位置に `{ name: T, .. }` を書ける
  - 例: `props: { todo: Todo, onToggle: (id: i32) => () }`
- 型の同一性は**構造的**: フィールドの名前の集合と、それぞれの型が同じなら同じ型(順序は問わない)
  - 名前付きのレコード型(`type Todo = {..}`)は今までどおり名前で区別し、無名のレコードとは別の型
- **幅の部分型はなし**: `{ a, b }` を `{ a }` に渡すことはできない(余分なフィールドもエラー)
  - TS とは違うが、`.d.ts` の側では TS の構造的な型になるので、TS から渡す分には TS の規則で通る

### B. 無名のレコードの値

- `{ todo: t, onToggle: f }`(フィールド名と `:` で始まる `{` は、ブロックでなくレコード)
- 省略形の書き方
  - `{ todo, onToggle: f }`: 最初のフィールドが `name:` の形なら、残りに省略形を混ぜてよい
  - `{ todo, onToggle }` だけは、ブロックとの区別のため書けない。`{ todo: todo, .. }` か、型名を付けて書く
- **期待する型が名前付きのレコードなら、その型の値になる**: `let t: Todo = { id: 1, title: "a", done: false };`
  - typed array の「型で明示的に作る」と同じ規則
  - `Todo { .. }` と書く必要がなくなる
- 更新: `{ done: true, ..t }` は、`t` と同じ型になる(無名でも名前付きでも)

### C. TS 風の関数型

- `(id: i32) => ()` を型の位置に書ける
  - 引数の名前は省略できる: `(i32) => ()`
- 引数の名前は、`.d.ts` と、エディタの hover / シグネチャの表示に使う
- `fn(i32) -> ()` も当面は受け付けるが、ドキュメントと例は TS 風に揃える(将来は警告)

### D. コンポーネントの props

- コンポーネントは `fn Name(props: { .. }) -> Jsx`
- JSX の props の検査は、名前付きのレコードと同じ(足りない prop、余分な prop、型の違いはエラー)

## 変更前 / 変更後(corpus/components.zsc)

```zsc
// 変更前
type TodoItemProps = { todo: Todo, onToggle: fn(i32) -> () }
pub fn TodoItem(props: TodoItemProps) -> Jsx { .. }

type Stats = { total: i32, done: i32 }
pub fn stats(todos: Array<Todo>) -> Stats {
  Stats { total: todos.len(), done: todos.filter((t) => t.done).len() }
}
```

```zsc
// 変更後
extern "react" fn useState<T>(init: T) -> (T, (next: T) => ()) = "useState";

type Todo = { id: i32, title: String, done: bool }

pub fn TodoItem(props: { todo: Todo, onToggle: (id: i32) => () }) -> Jsx {
  <li className={if props.todo.done { "done" } else { "" }} onClick={() => props.onToggle(props.todo.id)}>
    {props.todo.title}
  </li>
}

pub fn TodoList(props: { todos: Array<Todo>, onToggle: (id: i32) => (), title: String }) -> Jsx {
  <section>
    <h2>{props.title}</h2>
    <ul>{props.todos |> .map((t) => <TodoItem key={t.id} todo={t} onToggle={props.onToggle} />)}</ul>
  </section>
}

pub fn App(props: { initial: Array<Todo> }) -> Jsx {
  let (todos, setTodos) = useState(props.initial);
  <TodoList title="todo" todos={todos} onToggle={(id) => setTodos(toggle(todos, id))} />
}

fn toggle(todos: Array<Todo>, id: i32) -> Array<Todo> {
  todos |> .map((t) => if t.id == id { { done: !t.done, ..t } } else { t })
}

pub fn stats(todos: Array<Todo>) -> { total: i32, done: i32 } {
  { total: todos.len(), done: todos.filter((t) => t.done).len() }
}
```

注意: `if c { { done: .., ..t } }` の二重の波括弧は、ブロックの中のレコードの値。読みにくいので、評価で意見を聞きたい(代替案 2)。

## 出力

JS は今と同じ(プレーンなオブジェクト)。`.d.ts`:

```ts
export declare function TodoItem(props: { readonly todo: Todo; readonly onToggle: (id: i32) => void }): ReactElement;
export declare function stats(todos: readonly Todo[]): { readonly total: i32; readonly done: i32 };
```

- `@wasm` では無名のレコードは使えない(Rust の struct に名前が要る)
  - エラーにして、`type` で名前を付けるように促す
  - 将来、フィールドの集合から名前を作ることもできる

## 代替案

1. **TS と同じく幅の部分型を入れる**(`{ a, b }` を `{ a }` に渡せる)
   - TS の書き方にはより近い
   - 一方で、型推論(単一化)が部分型を扱う必要があり、実装と誤りにくさが落ちる
   - 選ばない
2. **無名のレコードの値に印を付ける**: `#{ total: .., done: .. }` や `.{ .. }`
   - ブロックとの区別が先読みなしで決まり、`if c { #{ .. } }` も読みやすい
   - TS の見た目からは離れる
   - 評価で意見を聞きたい
3. **props だけ特別扱い**(コンポーネントの引数に限って無名を許す): 一般性がないので選ばない

## 未決事項

- `{ todo, onToggle }`(全部が省略形)をブロックとどう区別するか
  - 案: 期待する型がレコードならレコードとして読む。ただし構文解析が型に依存する
- 任意のフィールド `title?: String`(TS の optional)を `Option<String>` として入れるか
  - JSX の props で特に欲しくなる
- 無名のレコードの型が長くなったときの、エラーメッセージでの表示(型の別名を表示に使う)

## 改訂履歴

(まだなし)
