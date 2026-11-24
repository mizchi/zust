# zustscript のターゲット属性とビルド（設計方針）

同じパッケージに、共通の計算、DOM を操作する関数、Rust のメモリを直接扱う関数、Wasm SIMD カーネルを書けるようにする。**属性は関数本体をどこに生成できるかを決め、ビルドはどの公開関数を入口にするかを決める。** JS から Wasm を呼ぶために生成する JS ラッパーは、関数本体の JS 実装には数えない。

ターゲットを属性と公開入口で選ぶ方針を採用する。以下の `@rust`、`@js`、`--entry`、`--target` は実装予定の構文・オプションである。

## 関数本体の出力先

| 宣言 | 本体を書く言語 | 生成先と利用できる API |
| --- | --- | --- |
| 通常の `fn` | zustscript | JS または Rust。呼び出し側の出力先に合わせて生成する。その出力先で扱えない構文や依存があれば、呼び出し元を示してエラーにする。 |
| `@js fn` | zustscript | JS のみ。型付きの JS 外部宣言を通じて DOM ラッパーなどを使える。DOM の有無は実行環境の条件なので、ブラウザ向け依存は Node 向けの入口から外す。 |
| `@rust fn` | Rust | Rust のみ。シグネチャも本体も Rust として検査する。Rust 内の呼び出しは Rust ABI のままにする。 |
| `@wasm fn` | zustscript | 現行仕様を引き継ぐ。ブラウザ向けには Rust 経由で Wasm になり、JS に型付きの呼び出し口を出せる。`--emit-rust` では Rust 関数として使える。 |
| `@simd fn` | zustscript の数値カーネル | 実行先は Wasm に固定する。通常は SIMD 命令を生成する。既存の `--no-simd` はスカラー **Wasm** を明示的に選ぶ比較用設定として残す案とする。`@simd(relaxed)` の数値上の許容範囲は現行の規則に従う。 |

出力先の属性は一つの宣言に一つとする。`@js @rust` のような組み合わせは宣言位置でエラーにする。共通の計算なら属性を外し、両方からその関数を呼ぶ。

## 同じソースからビルドを分ける

たとえば計測値の二乗和を表示するパッケージでは、表示を担当する入口と、Rust から使う計算の入口を分けられる。以下は提案構文の例。JS ラッパーの `showEnergy` は DOM 要素に計測値を表示する。

```zustscript
extern "./dom.js" fn show_energy(x: f32) = "showEnergy";

@simd fn energy(xs: F32Array) -> f32 {
  xs.fold(0.0, (sum, x) => sum + x * x)
}

@js export fn render(xs: F32Array) -> () {
  show_energy(energy(xs))
}

export fn sum(xs: Array<i32>) -> i32 {
  xs.fold(0, (total, x) => total + x)
}
```

| ビルド例 | 選ばれるもの | 期待する結果 |
| --- | --- | --- |
| `zsc app.zsc --target web --entry render` | JS の `render` と DOM ラッパー、Wasm の `energy` | JS、`.d.ts`、SIMD を含む `.wasm`。配列を渡す境界は生成 JS で確認できる。 |
| `zsc app.zsc --target rust --entry sum` | `sum` と、その Rust 実装が使う型・関数 | Rust モジュール。DOM ラッパーの解決や SIMD モジュールの生成は不要。 |
| `zsc app.zsc --target js --entry render` | `render → energy` | `energy` は Wasm を必要とするためエラー。JS 版を自動生成すると、配布物と浮動小数点の計算順が変わりうる。 |

入口を省略した場合は従来どおり全 `export` を要求する。ビルドが成功したのに公開したはずの関数が消える、という挙動を避けられる。`--entry` は複数指定でき、そのビルドで選んだ入口だけを公開する。パッケージ内の構文・名前・共通の型は先に検査し、ターゲット固有の依存解決とコード生成は、入口から到達する関数について行う。呼ばれる通常関数の生成先は呼び出し元から伝播させる。JS と Rust の両側から使う場合は両方で検査する。

JS から `@wasm` と `@simd` を呼ぶ経路だけが、ここで定義する自動の JS → Wasm 境界である。`@js` から `@rust` の直接呼び出しはエラーにする。ブラウザから公開したい Rust 処理には、境界で渡せる型を持つ `@wasm` の入口を用意する。その入口からは wasm32 でビルドできる Rust 実装を呼べる。`@rust` コードから DOM への暗黙の呼び戻しも作らない。たとえば通常関数 `report` を経由して DOM に到達した場合は、`sum → report → show_energy（JS 外部関数）` のように最初の不適合と呼び出し経路を報告する。

## Rust の表現力を保つ境界

`@rust` の中で参照を配列に置き換えたり、借用した結果を自動で `Vec` にしたりすると、所有権も計算量も変わる。このため **通常の `.rs` ファイルを Rust モジュールとして取り込む方法を第一にする**。提案構文 `@rust mod buffers;` は隣接する `buffers.rs` を参照する。実装は利用側の crate から通常のモジュールとしてコンパイルできるようにし、Cargo の依存、edition、feature、マクロ解決の文脈を引き継ぐ。

たとえば次の関数は一つのバッファを二つの可変領域に分ける。Rust 利用者は両方をその場で更新でき、結果のどちらも入力より長く生存できない。

```rust
pub fn regions<'a, T>(
    samples: &'a mut [T],
    mid: usize,
) -> Option<(&'a mut [T], &'a mut [T])> {
    if mid > samples.len() {
        return None;
    }
    Some(samples.split_at_mut(mid))
}
```

このシグネチャに zustscript で別の型定義を書く必要はない。Rust の利用者にはモジュールの `pub` 項目をそのまま公開する。trait と関連型、ライフタイム、`unsafe fn`、条件付きコンパイルも rustc が検査する。短い処理用のインライン `@rust fn` は同じ Rust 項目を生成する糖衣構文とし、完全な Rust 構文を扱えるまでは外部モジュールを正式な書き方にする。

通常の zustscript から呼びたい関数には、zustscript が表現できる引数・戻り値の型を持つ安全なアダプターを公開する。JS にも公開するなら、さらに Wasm 境界で表現できる型に限定する。`regions` が返す二つの `&mut [T]` は Rust 内に留め、たとえば「左半分を更新し、更新件数を返す」という操作を一度の呼び出しにまとめる。`unsafe` の事前条件とバッファの所有者は Rust 側で管理する。

たとえば次のインライン構文案では、`count_equal` は Rust のイテレータを使い、Rust 内ではスライスを借りて呼べる。`count` だけを web の入口にすると、JS の配列を Wasm に渡す処理は `count` のラッパーに一度だけ入る。`count` から `count_equal` は同じ Wasm モジュール内の Rust 呼び出しとする。

```zustscript
@rust fn count_equal(xs: &[i32], needle: i32) -> i32 {
    xs.iter().filter(|&&x| x == needle).count() as i32
}

@wasm export fn count(xs: Array<i32>, needle: i32) -> i32 {
  count_equal(xs, needle)
}
```

`@rust` は Rust バックエンドの指定で、ネイティブ CPU 限定の指定とは区別する。wasm32 で利用できる Rust コードもある。フル Rust の保証は Cargo/rustc 経路に置く。現在の `zustc` が扱う Rust の範囲を超えた場合は、対応するツールチェーンが必要だと診断する。ホスト OS 固有の crate を Wasm 向けに使えるという保証はしない。

## バイト列を JS と共有する

ブラウザで受け取ったバイナリの一部を Rust に渡す場合、JS の公開型は標準の `Uint8Array` にする。JS 内では元のバッファを共有できる。Wasm へ渡すときだけ、そのビューが指す範囲を線形メモリにコピーする。変換用クラスの生成や、レコード用のシリアライザの読み込みは不要になる。

| 境界 | バイト列の渡し方 | 所有者とコピー |
| --- | --- | --- |
| JS → JS | `asBytes(view)` | 同じバッファの同じ範囲を指す。コピーは 0 回。片方からの書き込みはもう片方からも見える。 |
| JS → Wasm | 呼び出し口が Rust 用の領域を確保し、`writeBytes(memory, offset, view)` で転送する | 入力を 1 回コピーする。Rust はその領域を呼び出し中だけ `&[u8]` として借りる。 |
| Wasm → JS | Rust が返した領域を解放する前に `readBytes(memory, offset, length)` で取り出す | 結果を 1 回コピーする。JS は次の Wasm 呼び出しや `memory.grow()` の後も値を保持できる。 |
| Rust → Rust | 通常の `&[u8]`、`&mut [u8]`、`Vec<u8>` | 借用と所有権の移動を Rust のシグネチャで選ぶ。JS 用の変換は入らない。 |

JS 側の試作はローカルパッケージの独立した入口に置いた。[zustscript-helper/bytes の実装](../helper/ts/bytes.js)

```js
import { asBytes } from "zustscript-helper/bytes";

const packet = Uint8Array.of(2, 0x41, 0x42);
const payload = asBytes(new DataView(packet.buffer, 1, 2));
payload[0] = 0x61; // packet[1] も 0x61 になる
```

`asBytes` は `byteOffset` と `byteLength` を保つので、Node の `Buffer.subarray()` や `Float32Array` の一部も渡せる。多バイトの要素はメモリ上のバイト順で見える。ファイル形式などでバイト順が決まっている値には `DataView` の読み書きを使う。

Wasm 側にその領域を確保し、Rust の借用が終わるまで再利用しない責任は呼び出し口にある。ヘルパーはアロケータを持たない。Cargo を使う `@rust mod` で JS オブジェクト自体が必要になれば、既存の `js_sys::Uint8Array` を明示的に選べる構成を候補とする。`copy_to` は Rust のスライスへコピーし、`unsafe fn view` は Wasm のメモリ拡張で無効になりうるビューを返す。このため自動生成する公開 API には所有された戻り値を採用する。[js-sys の Uint8Array](https://docs.rs/js-sys/latest/js_sys/struct.Uint8Array.html#method.view)

### 配布サイズと採用範囲

未使用のバイト列処理による配布サイズの増分は 0 B を条件とする。JS 単独の入口には Wasm ローダーを生成せず、Rust 単独の入口にも JS バインディングの依存を加えない。バイト列を使う入口では必要な操作だけを選び、既存の型付き配列用ラッパーにもある `.set()` と `.slice()` を利用する。

試作では、同じ Rust 関数に渡す手書きの JS とヘルパー経由の JS を比較した。Node 24.22.2、esbuild 0.28.2 の ESM バンドル（minify）、gzip レベル 9、rustc 1.100.0 の `wasm32-unknown-unknown` 出力での値。サイズはファイルごとのバイト数。

| 配布物 | バイト数 | gzip 後 |
| --- | ---: | ---: |
| JS のみ、ビューの長さを返す | 41 | 61 |
| 同じ JS に未使用のヘルパー import を追加 | 41 | 61 |
| `asBytes` だけを公開する JS | 94 | 108 |
| Wasm を呼ぶ手書きの JS | 267 | 197 |
| 同じ処理をヘルパー経由で呼ぶ JS | 376 | 231 |
| 両者が呼ぶ同一の Rust Wasm | 140 | 143 |

1 か所だけで使う例では共通化のコストは gzip 後 **34 B**。この測定は固定長の領域を使う Rust の小さな関数とその JS 呼び出し口を対象にしている。zsc のローダーや実アプリのアロケータを含む合計サイズは、言語への統合時に別途測る。再現するには `wasm32-unknown-unknown` ターゲットと esbuild を用意し、リポジトリのルートで `just --justfile crates/zustscript/bench/bytes/justfile check` を実行する。[測定コード](../bench/bytes/check.mjs)

現時点で使えるのは JS ヘルパーである。次のコンパイラ実装では、手書き `wasm fn` の引数 `&[u8]` と戻り値 `Vec<u8>` に対し、生成する `.d.ts` の両側に `Uint8Array` を出す。その経路で部分配列、空入力、解放後も保持する戻り値を検査し、同じ入口の JS と Wasm の増分をそれぞれ記録する。

## 現行仕様からの移行と受け入れ条件

- 現在の `--all-js` は `@simd` を JS に、`--emit-rust` はスカラー Rust に変換できる。提案では、選択した入口が `@simd` に到達する場合にどちらもエラーとする。JS・ネイティブ Rust でも必要なアルゴリズムには通常の `fn` で実装を用意する。既存の SIMD 差分テストは、その実装との比較を維持する。
- 既存の `wasm fn` と `wasm { ... }` は、当面 Wasm 用の手書き Rust として互換性を保つ。`@rust` への一括置換は、JS に公開していた境界を変えてしまう。JS 単独ターゲットが手書き `wasm fn` に到達した場合はエラーとする。
- 同じソースから上の三つのビルドを試す。web 版では JS から `render` を呼んで表示まで確かめ、Rust 版は DOM の依存を置かずにビルドする。JS 単独版の失敗時には呼び出し経路と `@simd` の宣言位置を確認する。
- Rust の検証では `regions` の左右を書き換えて元の配列に反映されることと、結果を入力の寿命より外へ持ち出す例を rustc が拒否することを確かめる。境界検査では、この借用をそのまま JS に公開しようとした宣言を拒否する。

最初の実装候補は、入口から到達する関数と生成先の決定、`@simd` の Wasm 固定、`@js` と既存の外部宣言の検査、`@rust mod` の Cargo 連携の順とする。インライン Rust の字句解析と、ターゲットごとに同名の関数を差し替える仕組みは別に検討する。
