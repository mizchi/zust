# 回帰監視が見つけたもの

日付: 2026-11-15

サイズ予算・criterion・`tests/compare.sh` の回帰監視 (`bench/regress.sh`) が実際に働いた例。どれも予算や閾値を緩めずに直した。

- サイズ
  - enum の追加で要約が 1 byte/アイテム増えた。要約の符号化を詰めて解決した。
  - 検査済みの印が 4 byte/関数増えた。空なら書かないようにした。
  - prelude を全部付けていたので、どのキャッシュも約 2.3 KB 増えた。使われるものだけを付けるようにした。
  - フィールド名 `min` / `max` で `core::cmp` の関数まで付いていた。`core` という識別子が現れたときだけにした。
- 速度
  - no_std 対応で、LSP の 1 関数編集が +31%、no-op ビルドが +10% 遅くなった。
  - 時間帯によるぶれを除くため、ベースラインのコミットを worktree で同時にベンチし、callgrind で原因を詰めた。
  - 直したもの：毎回の全識別子のハッシュ（prelude の選択）、LSP が本文全体をハッシュしていたこと、名前表の SipHash。
  - 名前表は自前の FxHasher に替えた。キャッシュのキーには使わない。
  - 結果、LSP の 1 関数編集は −7%、no-op ビルドは −4% と、ベースラインより速くなった。
- 正しさ（クロージャ・dyn の追加時。`tests/compare.sh` に「キャッシュなし」と「キャッシュからの再ビルド」の両方を比べる段を足して見つけた）
  - 同じ関数の中のクロージャは、struct の deep hash（＝親アイテムのハッシュ）とシグネチャがどれも同じになる。そのため `consume::<closure#8>` と `consume::<closure#9>` のキャッシュキーが衝突し、2 回目のビルドで別のクロージャの機械語が使われていた。キーにクロージャの名前を入れて直した。
  - vtable を zero-init で定義したため `.bss` に置かれ、リロケーションが適用されず null を呼んでいた。
- 速度（クロージャ・dyn の追加時）
  - HEAD との A/B で、LSP の 1 関数編集に +4% が出た。型推論の呼び出しごとの `Vec` の clone と、struct のハッシュに名前を入れていたことが原因で、どちらも外した。
  - callgrind の命令数で見ると、1 関数編集（`Session::check`）は HEAD より少なくなった。didOpen（cold）は約 +2% で、criterion でも有意差はない。
- 関数ポインタ・Box の追加時
  - `Ty` が 16 → 24 byte になっていた（前回の `Dyn(&'static str)` が fat pointer だったため）。`&'static String` にして 16 byte に戻した。
  - criterion では LSP の 1 関数編集が +14%、lexer が +10% と出た。ただ lexer のコードは変えていない。
  - callgrind で見ると、`lexer::St::tok` の `b"..".contains(&c)` が memchr になっていた。呼び出し回数は同じで、命令数だけ +32% 増えていた（静的な文字列の配置の違い）。
  - `matches!` に替えた結果、ベースラインより LSP の 1 関数編集 −5%、no-op ビルド −13%、lexer −6% になった。
- Drop の追加時（`tests/compare.sh` に valgrind の段を足した：メモリエラーと確実なリークがあれば失敗）
  - `*b` の読み出しが `b` のムーブ扱いになり、Box がリークしていた。deref の中身はムーブせずに読むようにした。
  - `self.top = n.next`（Box の中身のフィールドだけのムーブ）で、`n` 全体をムーブ扱いにしていたため、ノードがリークしていた。
    ムーブしたフィールドを 0（null の Box / `None`）で埋め、持ち主は普通に drop するようにした。
  - 以前からあった不具合も 2 つ見つかった。
    - `&Rect { .. }` を `&'static` で返すと、スタック上の一時値を指していた。rustc と同じく定数式は静的データに昇格するようにした。
    - タプル構造体のフィールド `n.0` の型を推論していなかった。
  - Box も Drop も使わないプログラムは、drop の処理を一切生成しない（`Program::has_drop`）。速度とサイズ予算は変わらない。
- Vec / String の追加時
  - criterion ではほぼ全項目が +20〜40% になったが、変えていない lexer も同じだけ遅く、マシンの負荷による揺れだった。
    同じ時間帯に HEAD と並べて測ると ±10% 程度の揺れの中。callgrind の命令数も HEAD 以下。
  - 本物の増加は prelude の選択（+26%）だけだった。prelude の名前が増え、先頭 1 byte ごとの候補表が長くなったのが原因。(先頭 1 byte, 長さ) で引く表にして HEAD より少なくした。
  - `==` は識別子として現れないので、prelude の `Vec::eq` / `String::eq` が付かず、ポインタを比べていた。eq は型と一緒に付け、到達可能性の根にも入れた。
- Iterator の追加時
  - サイズ予算がほぼ全部の例で超過した（hello のキャッシュが 3.2 KB → 10.8 KB）。`for x in v.iter()`・変数名の `count`・`a.max(b)`・`Option::take`・フィールドの `.next` で、イテレータの部品が丸ごと付いていた。
    付ける条件を「イテレータを作る呼び出しの後ろにアダプタ・消費メソッドが続く」などに絞り、既存の例の予算は 1 byte も緩めずに通るようにした。
  - LSP の 1 関数編集で prelude の選択が +63%（候補の名前が増えたため）。アイテムのハッシュごとに結果を Session に覚え、変わっていないアイテムは走査しないようにした。callgrind で `Session::check` は HEAD より少ない。
  - 型推論が一時的に parse したマクロの引数の型をアドレスで記録していたため、解放後のアドレスに codegen が作ったノードが重なり、別の式の型（`&str`）を引いていた。一時ノードは記録しないようにした。
  - クロージャを 1 つ編集すると、それを受け取る map などのインスタンスが 65 個作り直されていた。クロージャの struct の deep hash から親アイテムのハッシュを外し、18 個（親とクロージャだけ）にした。
- format! / HashMap の追加時
  - nostd_core のキャッシュが 8.7 KB → 22.7 KB になった。生ポインタの `p.write(v)` の `write` と `a.max(b)` で、書式化の部品と String / Vec が丸ごと付いていた。
    書式化のマクロ名は `name!` の形のときだけ数え、`__exr_str_cmp` も max / min では付けないようにした。
  - iterators は HashMap のイテレータ（`HmIter` など、`impl Iterator for` の側）が付いていた。これらも HashMap の名前があるときだけにした。
  - 書式化の helper（`__exr_fmt_int` など）はジェネリック `<G>` にして根から外した。呼ばれた分だけインスタンスとしてコンパイルされる。
    instance の本体から呼ばれる非ジェネリックな関数（`__exr_print` → `putchar`）は、codegen の各波の後に生きている集合を広げて拾う。
  - 予算を緩めたのは vec_string のキャッシュだけ（45.0 KB → 48.5 KB）。`name.to_string()` は受け手の型が選択の時点で分からず（整数なら Display が要る）、書式化の部品を付けるため。
  - callgrind: gen1000 のビルドは +0.15%、check は −4%。prelude を多く使う例は増えた（iterators +5%、vec_string +15%。付く prelude が増えた分）。
- 大小比較（PartialOrd / Ord）の追加時
  - 比較の glue（`__exr_cmp`）を常に付けると、小さい例のキャッシュが 20 byte ずつ増えて予算を超えた。
    再帰する型は Box / Vec を通るので、glue はそれらを使うプログラムにだけ付け、ほかは codegen が比較をその場で展開するようにした。
  - `use core::cmp::{max, min}` の `cmp` と境界の `T: Ord` で、trait の宣言と `Ordering` が付いていた。`cmp::` はパスとして数えず、trait の宣言は手書きの `impl Ord for` があるときだけ付ける。既存の予算は 1 byte も緩めていない。
  - 途中で見つけた既存のバグ
    - ブロック末尾の式や match の arm の `println!` の一時値を文の終わりで drop していて、Cranelift の検証（値の支配関係）に落ちていた（`if c { println!(..) } else { .. }`）。
      末尾の式と arm の一時値は、その場で drop するようにした（Rust 2024 と同じ）。
    - `&&&str` の `len()` が 1 段しか辿らず、0 を返していた。
    - 比較の右辺の `None` / `Ordering::Less` を場所（ローカル変数）として読もうとしていた。
  - callgrind: gen1000 のビルドは −2.6%、check は −6.8%、prelude を使う例は +0.4〜1.5%。
- マージソート・binary_search・clamp の追加時（`examples/sorting.rs`）
  - sort は 16 個ずつの挿入ソート + 下から上へのマージ（作業領域は realloc で n 要素分）。要素は `read` / `write` でビット移動するので Drop を持つ型でも二重解放しない（valgrind で確認）。
    20 万要素の `Vec<u64>` の sort が 56 秒 → 0.04 秒になった。
  - sort を使う例はサイズ予算を上げた（vec_string +3%、ordering +16% など）。マージソートの本体が挿入ソートより大きく、要素型と比較のクロージャごとにインスタンスができるため。
    vec_string のビルドも +9%（callgrind。コンパイルする関数が増えた分）。sort を使わないプログラムは変わらない（gen1000 は ±0%）。
  - 途中で見つけた既存のバグ
    - `&mut u64` / `&u64` を受け手にした整数のメソッド（`s.wrapping_mul(..)`）が、指す先ではなくアドレスを計算していた。
    - `std::cmp::Ordering::Less` のような 3 段以上のパスのバリアントを、2 段目の名前（`cmp`）で探していた。
    - 生ポインタの `==` / `!=` が使えなかった。
- BinaryHeap・sort_by_cached_key の追加時
  - サイズ予算は既存の項目をどれも変えていない（BinaryHeap は名前があるときだけ付く）。
  - callgrind: prelude が大きくなった分、プロセスごとに 1 回の prelude の字句解析が +0.5M 命令（hello で +3.8%、約 0.1 ms）。gen1000 のビルドは ±0%。
    check の計測は 1000 関数で ±8% ほど揺れる（HEAD 同士でも 1.82G〜1.92G）ので、3 回ずつ測って差がないことを見た。
- BinaryHeap の append / extend の追加時
  - 型推論が `Vec<T>::extend` の `Item = T` を `v.extend(w.iter())` の `&i32` と単一化して、`v` を `Vec<&i32>` にしていた。extend は引数の要素型（参照なら中身。`&str` は除く）から v の要素型を決めるようにした。
  - 途中で見つけた既存のバグ：`&&str` の `to_string()` の型を推論できず、`a.iter().map(|s| s.to_string())` のクロージャの戻り値が決まらなかった。
  - サイズ予算・callgrind（gen1000 / hello / vec_string / binary_heap）はどれも ±1% 以内。
- よく使う機能をまとめて足した時（`tests/probe.sh`：`tests/probe/*.rs` の小さいプログラムを rustc と比べて通った数を数える。2 / 28 から始めた）
  - prelude が 4000 行近くになり、プロセスごとに 1 回の prelude の字句解析が hello のビルドの半分（8M 命令）を占めた。
    分割を build.rs に移し（`src/lexer.rs` をそのまま使う）、prelude の本体は `src/prelude_lib.rs.in` に置いた。hello は 14.9M → 9.3M 命令。
  - 文字列の helper・演算子の trait で、既存の例のサイズ予算が軒並み超えた（nostd_core のキャッシュ 8.7 KB → 29.7 KB など）。
    - 整数の `to_ascii_uppercase` や `Option::replace` と同じ名前の helper は、String か文字列リテラルの受け手があるときだけ付ける
    - 演算子の trait の宣言は手書きの `impl Add for ..` があるときだけ
    - String の新しいメソッドはジェネリック `<G>` にした（メソッド名だけで生きているとみなされ、Vec の `pop` などで String の `pop` までコンパイルされていた）
    - chars で回していた helper は UTF-8 を直接読み、イテレータの部品を付けない
    既存の予算は 1 byte も緩めていない。
- 浮動小数点・i128・標準入出力・Rc/RefCell・`impl Iterator` の戻り値などを足した時（probe を 73 本に増やし、すべて rustc と一致）
  - 10 進変換は glibc の `ecvt_r` が桁を間違える場合があったので、1e9 進の多倍長で正確に行う。`examples/float_fuzz.rs`（6000 行）・`examples/int128.rs` の乱数の照合が rustc と一致する。
  - サイズ予算は、新しい prelude の型を「名前が出たときだけ付ける」（owner の gating）にして、ほとんどの項目を変えずに済ませた。上限を上げたのは iterators（+68 B）・text_more（+0.4%）・hashmap（+0.3%、Iterator の既定メソッドの `cycle` の分）だけ。
  - callgrind（命令数）：hello は作業前 9.96M → 9.95M（prelude の項目表を作るときの `Vec` の確保をやめて、増えた分を打ち消した）。gen1000 は 2.43G で、HEAD のビルド同士でも ±2% 揺れる範囲で差はない。
- オーバーフロー検査の追加時
  - callgrind：gen1000 のビルドは 2.43G → 2.55G 命令（+5%）、`-O` は HEAD と同じ 3.06G。hello は 9.95M → 10.25M（main のハンドラ登録の分）。
    最初は検査ごとに panic 用の分岐とブロックを作っていて +50%（3.67G）だった。`trapnz` にし、定数の除数・シフト量の検査を省いた。
  - サイズ：検査の分だけコードが増える（gen1000 のバイナリ 443 KB → 498 KB、`-O` なら増えない）。rustc の debug ビルドと同じ性質の増加なので、予算は UPDATE_BUDGET=1 で取り直した。
  - 途中で見つけた既存のバグ：`?` で String のエラーを返すと二重解放、`{ return None; }` が `()` 型、enum の `==` が使っていないバリアントのポインタを辿る、`saturating_add` がベクトル用の命令、`struct Node { children: Vec<Node> }` が「無限の大きさ」、`Node<T>` が `Option<Box<Node<T>>>` を持つと具体化が止まらない、char のリテラルパターン、型推論の中で作り直した式のアドレスが codegen のノードと重なる、など。
  - `|a| a + 1` の戻り値を呼び先の境界 (`B`) で決めようとして、まだ決まっていない `B` を親の型推論より優先していた（`map(|w| w.parse().unwrap())`）。
- panic のメッセージを種類ごとに出した時
  - 実行時のハンドラ（表引き・数の書き出し・メッセージ）が std のプログラムに必ず付く分、小さい例のサイズが +3〜6 KB（hello のバイナリ 6.0 KB → 10.0 KB）。
    最初は +6〜7 KB で、書式つき panic の関数が String / Vec を引き込んでいた（書式化を使うプログラムだけに付けるようにした）・
    sigaction の構造体を配列の初期化で作っていた（static にした）・メッセージを if の連鎖で選んでいた（繋げた文字列と位置の表にした）のを直した。
  - 表は最初 1 か所 16 byte で gen1000 のオブジェクトが +15%。オフセット 20 bit・メッセージ番号 13 bit・種類と行と列 31 bit を 1 語に詰めて +8%（キャッシュも同じ形で +5%）。予算は UPDATE_BUDGET=1 で取り直した。
  - callgrind：gen1000 のビルドは 2.54G → 2.57G（+1%）、`-O` は 3.06G → 3.11G。hello のキャッシュなしのビルドは 10.3M → 29M（ハンドラの関数の parse と codegen。約 4 ms）、キャッシュありの再ビルドは 2.8M → 5.6M。
  - 途中で見つけた既存のバグ：`__FmtError` が Debug を実装していなかった（`Result<_, fmt::Error>` の unwrap）。
- 型の違う `==` を PartialEq の trait で解決した時
  - 名前の付け方：右辺の型が self 型と違う `impl PartialEq<R> for S` のメソッドは `S::<PartialEq<R>>::eq`（Display / Debug の `S::<Display>::fmt` と同じ）。`==` は lt と rt が違うときだけその候補を探すので、同じ型の比較は今までどおり。
  - 型推論：`==` の両辺が Vec と配列・スライスなら要素どうしを、数のリテラルのときだけ単一化する（`v == [1, 2]` で v: Vec<i64>）。`Vec<String> == Vec<&str>` は単一化しない。
  - サイズ：配列を比べるプログラムに impl の分が付き、キャッシュは +0.1〜1.7%（enums +256 B など）。callgrind：gen1000・hello ±0%、vec_string +2%（付く prelude の parse）。
- assert_eq! の left / right の値を出した時
  - 書式化を使っていなかったプログラムに書式化の部品が付く（options のバイナリ 16.7 KB → 23.9 KB、ビルドは 57M → 125M 命令）。値を `{:?}` で出すのに要る分なので予算は取り直した。gen1000・hello は ±0%。
  - 途中で見つけた既存のバグ：
    - panic の場所の行がアイテムの中の行だった（2 番目以降の関数の行がずれる）。dbg! も同じだった。
    - prelude の名前を何も使わないプログラム（traits など）には panic のハンドラが付かず、panic が SIGILL（終了コード 132）だった。
    - `&None` を変数の借用として扱っていた。
- イテレータ・Vec・clone の追加時
  - **これまで `#[derive(Clone)]` の型の `.clone()` は使えず、`Vec<String>` の clone と `cloned()` はビットごとのコピー（二重解放）だった**。clone の glue を足して直した。
  - 新しいアダプタ（Rev / Peekable / FlatMap など）の `impl Iterator` が、イテレータを使うプログラムに全部付いていた。その struct が要るときだけにした。
    書式化の Formatter の `fill` フィールドが `Vec::fill` を引いて clone 一式まで付いていたので、フィールド名を変えた。既存の予算は緩めていない。
  - `vec![..].join(..)` の一時値の Vec を drop していなかった（valgrind で見つけた）。
  - callgrind: gen1000 ±0%、hello +2.5%、vec_string / iterators / hashmap は +0.5〜1%。
- マクロの引数の中のクロージャの追加時
  - 要約と codegen が `|` を含むマクロの引数だけを parse してクロージャを数える。callgrind は gen1000 +0.2%、iterators +0.7%、formatting ±0%。
  - 型推論は `vec![..]` の引数を parse し直したノードを推論の終わりまで持ち、クロージャの本体を呼び先の境界が分かるまで後回しにできるようにした（`vec![v.iter().map(|x| x * x).sum::<i32>(); 2]`）。
