# zustscript の計画メモ

README.md は「今あるもの」の説明として残す。このファイルは「これからやること」のメモ。

## 進め方の前提

- 試作は `crates/zustscript` の中で完結させる(zust のワークスペースには入れない独立した crate、CI も独立した workflow `.github/workflows/zustscript.yml`)。zust の crate には手を入れない(zustc は外部コマンドとして呼ぶだけ)
- vector IR(zust の IR へのベクタ型の追加)は [mizchi/zust#30](https://github.com/mizchi/zust/issues/30) として、**別のセッションで進行中**
  - zustscript 側では当面、`src/simd.rs` が v128 の wasm を自前で書く方式を続ける
  - #30 の V0(`Type::V128`、wasm バックエンド、`core::arch::wasm32`)が入ったら、`@simd` の出力先を次のどちらかにする
    - (a) 自前の v128 モジュールのまま
    - (b) `core::arch::wasm32` を使う Rust に落として zustc に渡す(`@wasm` と同じ経路)
    - (b) なら、zustc のキャッシュ・増分ビルドに乗り、`@wasm` と `@simd` を 1 つのモジュールにまとめられる。速さが自前モジュールの 1.5 倍以内(#30 の受け入れ基準)なら (b) に寄せる
  - `examples/kernels.zsc` / `examples/ml.zsc` と `tests/*_driver.mjs` は、#30 側の性能確認にも使える(同じソースの 3 方式の比較)
- Node は 26 系を使う

## TODO: ML 用途(文法の詳細は後で詰める)

優先度の高い順。

1. **wasm 側に置いたままの配列(ハンドル)**
   - 重みのように何度も使う配列を、呼び出しごとにコピーしない
   - 案: `WasmF32Array` のような型。作るときに 1 回だけ wasm のメモリへ置き、`@simd` / `@wasm` の引数にはオフセットだけを渡す
   - 決めること: 誰が解放するか(明示の `free`、スコープ、FinalizationRegistry)、JS 層から読み書きできるか、`@simd` のアリーナ方式との共存
   - 期待: 全結合層 512×512 の 0.14 ms のうち、大半を占める W のコピーが消える
2. **int8 の量子化**
   - `i8` と `I8Array`(`Int8Array`)
   - `@simd(relaxed)` で `i32x4.relaxed_dot_i8x16_i7x16_add_s` / `i16x8.relaxed_dot_i8x16_i7x16_s` を使う内積
   - 量子化と逆量子化(scale、zero point)の書き方
3. **比較とマスク**
   - カーネルの中の `if c { a } else { b }` を、比較命令と `v128.bitselect` にする
   - ReLU 以外の活性化関数(leaky ReLU など)、クリップ
4. **行列と行列の積**
   - 3 重のループ、タイル分割、転置
   - 1 次式のオフセットの範囲検査は、そのまま使える
5. **複数の出力、reduction の種類**
   - max / min の reduction、argmax、softmax(exp が要る: 近似か JS の Math.exp か)
6. **f16 / bf16**(必要になったら)

## enum の JS での表現(保留)

- 今は `TAG` を判別キーにしている: データなしの enum は `"All"`、位置引数のバリアントは `{ TAG: "Circle", _0, _1 }`、名前付きフィールドのバリアントは `{ TAG: "Circle", center, r }`
- TS との interop を小さくするために、enum ごとに表現を選べる `@tag(kind)` のようなディレクティブを検討する(既存の TS の判別共用体にそのまま合わせる)。後で決める
- 表現を決める処理は js.rs の 1 か所(`variant_value` / `field_key`)と wasm.rs の境界(`VTy::Enum`)に集めておく

## 文法の refine

[evals/syntax/](../evals/syntax/README.md) の改善ループで進めている。提案ごとの状態と、決めずに残したことは [TODO.md](../TODO.md) にまとめた。

- 提案になったもの: バリアントの修飾と名前付きフィールド(001)、無名のレコード型・分割代入・TS 風の関数型(002)、`Option` / `Result` の標準化と `?`・任意のフィールド(003)
- これから: or パターンと match のガード(004 の予定)、文字列の補間、モジュールと import
