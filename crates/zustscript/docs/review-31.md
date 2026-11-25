# PR #31 のレビュー指摘(CodeRabbit)の検証結果

[mizchi/zust#31](https://github.com/mizchi/zust/pull/31) に CodeRabbit が出した指摘 90 件の扱い。

- **直したもの(14 件 + 再レビューの 5 件)**: よく使う形で壊れた JS や誤った結果になるもの。248aa0d と 45d22c5 で修正し、examples/regress.zsc と tests/errors/ に回帰テストを足した
  - 番号: #3, #66, #68, #69, #72, #73, #74, #75, #76, #77, #80, #81, #82, #84, #24, #27
- **残り 76 件**: 248aa0d の時点で、1 件ずつ小さな .zsc の再現を zsc と Node 26 で実行して確かめた(下の表)。番号は CodeRabbit の指摘の通し番号、行番号は古いので内容で探す
- 修正は、このファイルを見ながら少しずつ進める(追跡用の issue にリンク)


File paths are relative to `crates/zustscript/`.

| # | file | severity | title | verdict | evidence |
|---|---|---|---|---|---|
| 39 | README.md | Minor | Include `f32` in the supported numeric types | valid | 決定事項の行は `i32`/`u32`/`i64`/`f64` だけで f32 がない(表には f32 がある) |
| 40 | README.md | Minor | Do not limit SIMD sum differences to the last digits | valid | @simd の `sum([1e16,1,-1e16,1])` は 2、JS は 1 |
| 41 | README.md | Minor | Put the effect example inside a function | valid | 例の `let who = handle ...` をそのまま書くと「expected an item, found `let`」になる |
| 42 | docs/plan.md | Minor | Mark named-field enum variants as planned | valid | `enum S { Circle { r: f64 } }` はパースエラーで、名前付きフィールドはまだない |
| 44 | examples/app.zsc | Minor | Reject nonpositive bucket counts | valid | `vec![0; buckets as usize]` と `buckets as usize - 1` の後に `h[k]` があり、0 や負の値は守られていない |
| 45 | examples/app.zsc | Minor | Return the real maximum for all-negative samples | valid | `fold(0.0, ..)` が初期値なので、負の値だけの標本では max が 0 になる |
| 46 | examples/app.zsc | Minor | Make sample selection keyboard-operable | valid | `<li onClick=..>` だけで、button も tabIndex もない |
| 47 | examples/app.zsc | Minor | Guard `stddev` against empty input | valid | `stddev` に空のときの分岐がなく、`s / 0` で NaN になる |
| 52 | examples/effects.zsc | Minor | List only `State` as an inferred effect of `counter` | valid | `counter` は greet を呼ばず、Get と tick だけなのに、コメントは Console も推論されると言っている |
| 55 | examples/numeric.zsc | Minor | Stop `escape` immediately for a negative limit | valid | `escape(0,0,-1)` は再帰が止まらず RangeError(Maximum call stack)になった |
| 54 | examples/shapes.zsc | Minor | Clarify that record/enum results use linear memory | valid | 戻り値は wrapper が out 領域(線形メモリ)に書き、DataView で読む(wasm.rs の Value の分岐) |
| 43 | src/ast.rs | Minor | Use a supported numeric type in the `TyS` example | valid | コメントが今も「`Int`, `Array<T>` ...」になっている |
| 32 | src/check.rs | Minor | Check continuation calls relative to the handler arm | valid | `let run = () => handle perform Ping() { Ping, k => k(()) }` が「k is called inside a closure」で拒否される |
| 33 | src/check.rs | Minor | Check coverage across nested pattern payloads | valid | `Value(true)` と `Value(false)` で網羅しているのに「`Value` not covered」になる |
| 34 | src/check.rs | Minor | Apply the expected record type before inferring fields | valid | `Box { value: [0.5] }` が「expected Box<F32Array>, found Box<Array<{float}>>」になる |
| 35 | src/check.rs | Minor | Substitute the component's concrete prop type args | valid | `<Show name="ok" />` が「expected T, found String」になる |
| 36 | src/check.rs | Minor | Align handler validation with the one-effect rule | valid | 別々の effect の Ping と Pong を 1 つの handle で受けてもエラーにならず、実行できる |
| 37 | src/check.rs | Minor | Reject duplicate enum definitions | valid | `enum E{A}` の後に `enum E{B}` を書き、`A` を使うと check.rs:1318 の unwrap で panic する |
| 38 | src/check.rs | Minor | Range-check integer literals after type resolution | valid | `-> i32 { 2147483648 }` が通り、JS に `return 2147483648;` がそのまま出る |
| 63 | src/check.rs | Major | No effect inference on duplicate fn definitions | valid | effect を持つ `f` と純粋な `f` を並べると、zsc が終わらない(20 秒でタイムアウト) |
| 64 | src/check.rs | Major | Require a numeric bound admitting the literal | partly | `T: Num` で `1.5` を返すと `value$i64` が `return 1.5` になる(誤り)。整数リテラルを float の実体で使う側は、出力に問題がない |
| 65 | src/check.rs | Major | Resolve lexical bindings before attributing effects | valid | ローカルの `let ping = () => ()` を呼ぶだけのコンポーネントが「performs Ping」で拒否される |
| 67 | src/check.rs | Major | Check JSX children against the declared `children` prop | valid | `children: i32` が必須なのに、`<Show />` も `<Show>hello</Show>` も通る |
| 48 | src/js.rs | Minor | Emit a valid JS value for f32 overflow | valid | f32 の `1e39` が `inf` と出力され、呼び出すと「inf is not defined」になる |
| 49 | src/js.rs | Minor | Keep the record key when renaming its value binding | invalid | 今は `{ default: default$ }` と出力され、結果も正しく 5 になる |
| 50 | src/js.rs | Minor | Detect numeric constructor / user export conflicts | valid | `pub fn i32` を書くと「Identifier 'i32' has already been declared」でモジュールが読み込めない |
| 51 | src/js.rs | Minor | Emit typed-array constructors as function values | valid | `let make = F32Array::zeros; make(2)` が実行時に「zeros is not defined」になる |
| 70 | src/js.rs | Major | Emit numeric patterns with their checked type | valid | `f(9007199254740993n)` が 3 になる(正しくは 1)。f32 の 0.1 も 0.1 のパターンに一致しない |
| 71 | src/js.rs | Major | Prevent nonterminating specialization | valid | `grow(n - 1, [x])` の例で zsc が終わらない(タイムアウト) |
| 78 | src/js.rs | Major | Declare bounded effectful exports as generators | valid | JS は `function* f$i32` なのに、.d.ts は `f$i32(x: i32): i32` になっている |
| 79 | src/js.rs | Major | Bind type params numeric specialization leaves generic | valid | .d.ts が `keep$T$i32(value: T, n: i32): T` になり、T が宣言されていない |
| 1 | src/lower.rs | Major | Do not move an owned array on alias | valid | `let ys = xs; xs[0]+ys[0]` で zustc が「use of moved value: `xs`」を出す |
| 2 | src/lower.rs | Major | Distinct Rust symbol per wasm specialization | valid | `f<i32>` と `f__i32` が衝突し、`g(1)` が 2 になる(正しくは 102) |
| 83 | src/lower.rs | Major | Preserve array representation through conditionals | valid | `if b {xs} else {xs}` を返すと「expected Vec<i32>, found &[i32]」になる |
| 85 | src/lower.rs | Major | Substitute type args inside nested records/enums | valid | `Wrap<i32>` の中の `Leaf<T>` で「T cannot be inside a record ...」になる(検査は通る) |
| 86 | src/lower.rs | Major | Keep the iterated array alive when the loop reassigns it | valid | `for x in xs.iter()` の中で `xs` に代入するコードを zustc が受け入れ、wasm は 203、JS は 10 になる(解放後の領域を読む UAF) |
| 87 | src/lower.rs | Major | Reject recursive types before expanding wasm layout | valid | `enum Chain { End, Link(Chain) }` を @wasm の引数にすると zsc が stack overflow で abort する |
| 88 | src/lower.rs | Major | Track borrowed arrays by lexical binding | valid | 内側のブロックで `let xs` を書いた場合と `let mut xs = xs` の場合の両方で、slice と Vec の型の不一致が zustc のエラーとして出る |
| 20 | src/parse.rs | Minor | Keep nested Rust comments out of brace counting | valid | ネストしたコメントの中の `}` で本体が切れ、「found `still`」のエラーになる |
| 21 | src/parse.rs | Minor | Parse callback argument lists by delimiter depth | valid | `&dyn Fn(i32,)` も `&dyn Fn(())` も境界の型として拒否される |
| 22 | src/parse.rs | Minor | Allow chained tuple projection | valid | `t.1.0` が「expected a field or method name, found `1.0`」になる |
| 23 | src/parse.rs | Minor | Reject a second record update base | valid | `R { ..first(), ..second() }` が `{ ...second() }` と出力され、first() が消える |
| 25 | src/parse.rs | Minor | Report EOF after a string escape | valid | 文字列の中の末尾の `\` で、parse.rs:50 の index out of bounds の panic になる |
| 26 | src/parse.rs | Minor | Recognize `>` as a delimiter in type/JSX contexts | valid | `let xs: Array<i32>=[1];` が「expected a name, found `>=`」になる |
| 28 | src/parse.rs | Minor | Scan Rust raw strings with full delimiter | valid | `r#"" }"#` で raw_braces が panic する |
| 29 | src/parse.rs | Minor | Leave `<` for comparison after a cast | valid | `x as i32 < 10` が「expected a name, found `10`」になる |
| 30 | src/parse.rs | Minor | Accept the signed i64 minimum | valid | `-9223372036854775808` が「integer literal out of range」になる |
| 31 | src/parse.rs | Minor | Reject boundary types in wrong direction at parse | valid | `String` の引数では wasm.rs:223 が panic、`&str` の戻り値では zustc が E0106 を出す |
| 12 | src/simd.rs | Major | Validate result elem type vs SIMD elem type | valid | F32Array の入力で F64Array を返すと、Float32Array の 16777216 が返る(JS は Float64Array の 16777217) |
| 13 | src/simd.rs | Major | Distinct names for memory and kernel exports | valid | カーネル名を `memory` にすると「Duplicate export name 'memory'」で CompileError になる |
| 14 | src/simd.rs | Major | Bounds check using wrapping i32 index arithmetic | valid | `xs[k*k+i]`(k=65536)で「index is 4294967296」の RangeError になる(JS は [5]) |
| 15 | src/simd.rs | Major | Resolve closure bindings before fn params | valid | 引数 `i=7` があると、@simd は [7,7]、JS は [0,1] になる |
| 16 | src/simd.rs | Major | Use JS identifier mapping for SIMD wrappers | valid | `@simd pub fn default` で、生成した JS が SyntaxError になり読み込めない |
| 17 | src/simd.rs | Major | Reject sum terms depending on the accumulator | valid | `fold(1.0, (s,x) => s+s)` で、@simd は 21、JS は 4 になる |
| 18 | src/simd.rs | Major | Finish reading JS inputs before writing to arena | valid | 要素の getter の中で `sum` を呼ぶと、`dot` が 200 になる(正しくは 5) |
| 19 | src/simd.rs | Major | Reserve inner-fold slot before visiting initializer | valid | 内側の fold の初期値に fold を書くと、@simd は [3,6]、JS は [6,6] になる |
| 58 | src/simd.rs | Minor | Document regrouping changes more than last bits | valid | `sum([MAX,-MAX,MAX,-MAX])` が @simd では NaN、逐次では 0 になる |
| 59 | src/simd.rs | Minor | Preserve the initializer when no vector chunk runs | valid | 空の配列の `fold(-0.0, ..)` が @simd では +0、JS では -0 になる |
| 60 | src/simd.rs | Minor | Internal names for wrapper params shadowing builtins | valid | 引数名を `Float64Array` にすると「Float64Array is not a constructor」になる |
| 61 | src/simd.rs | Minor | Align arena sizes without signed 32-bit math | valid | glue が `(b + 15) & ~15` で揃えるので、2^31 byte 以上では負の値になる |
| 4 | src/wasm.rs | Major | (CWE-125 out-of-bounds read, slice copy) | partly | `a.length` を確保、view、set、渡す長さで別々に読む。普通の配列なら安全で、長さが変わる array-like(宣言された型に反する入力)のときだけ範囲外の読み書きになる |
| 5 | src/wasm.rs | Major | Use the lowered Rust field name in record wrappers | valid | `type P = { type: i32 }` で zustc が「`ZsP` has no field named `type`」を出す |
| 6 | src/wasm.rs | Major | Reject unsupported boundary types in wrong direction | valid | `String` の引数では panic、`&str` の戻り値では wrapper が unit を返し、本体が `f(x)` だけになる |
| 7 | src/wasm.rs | Major | Release resources if argument preparation throws | valid | slice のコピーで TypeError を投げさせた後も、`$cbs.size` が 1 のまま残る |
| 8 | src/wasm.rs | Major | Use the JS emitter's identifier mapping in glue | valid | 引数名 `delete` で `function f(delete)` が SyntaxError になる(wasm fn と @wasm fn の両方) |
| 9 | src/wasm.rs | Major | Handle unit arguments without an ABI slot | valid | `x: ()` の引数で「unreachable: not an argument type」の panic になる |
| 10 | src/wasm.rs | Major | Keep wrapper params separate from source names | valid | `xs: &[i32], xs_n: i32` で zustc が E0415(`xs_n` の重複)を出す |
| 11 | src/wasm.rs | Major | Keep the boxed callback guard alive for the closure | partly | 今の zustc では guard が残り、保存した Box の呼び出しも 40 で正しく動く。rustc の 2021 版なら Copy のフィールド `f_h.0` だけを捕捉するので、guard が先に drop される |
| 56 | src/wasm.rs | Minor | Escape the Wasm filename as a URL path segment | valid | `demo#v2.zsc` から作ると、実行時に `out/demo` を開こうとして ENOENT になる |
| 57 | src/wasm.rs | Minor | Preserve a leading U+FEFF in returned strings | valid | Rust が `"\u{FEFF}x"` を返しても、JS では長さ 1 の "x" になる |
| 62 | tests/kernels_driver.mjs | Minor | Make a missing short-array error fail the driver | valid | 例外がなければ `err` は undefined のままで、examples.rs は 3 つの出力が一致するかしか比べない |
| 90 | tests/ml_bench.mjs | Trivial | Balance execution order across samples | valid | 毎回 simd、relaxed、no-simd、JS の固定順で計測している |
| 89 | tests/numeric_bench.mjs | Trivial | Counterbalance the benchmark execution order | valid | 毎回 `time(mk(w))` の後に `time(mk(j))` という固定順で計測している |
| 53 | tests/stub/react/jsx-runtime.js | Minor | Escape double quotes in HTML attribute values | valid | `esc` が置き換えるのは `&` と `<` だけで、`"` を置き換えていない |

## まとめ

74 件のうち、**valid 70 件、partly 3 件(#4、#11、#64)、invalid 1 件(#49、すでに直っている)**。

次に直すべきもの 5 件(どれもエラーにならず、黙って誤った結果やメモリ破壊になる):

1. **#86**: ループの中で反復中の配列に代入すると、zustc が通してしまう。解放後の領域を読む UAF になり、wasm と JS で結果が違う(203 と 10)。
2. **#2**: `f<i32>` と `f__i32` の Rust 名が衝突し、別の関数が黙って呼ばれる。
3. **#15 / #17 / #19**: @simd がクロージャの変数を正しく解決せず、JS と違う値を返す。3 件とも同じ解決処理の問題なので、まとめて直すとよい。
4. **#18**: アリーナが再入に弱い。入力を読む途中で別のカーネルが動くと、最初の入力が上書きされる。
5. **#70**: i64 と f32 のリテラルパターンが、検査した型の表現で出力されず、誤った arm に一致する。

zsc 自体が panic、無限ループ、stack overflow になるもの(#25、#28、#9、#31、#37、#63、#71、#87)も、まとめて診断を返す形に直す価値が高い。
