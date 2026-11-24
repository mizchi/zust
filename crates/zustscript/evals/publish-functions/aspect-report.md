# 判別 union の位置パターンで幅と高さを扱う

課題は TypeScript の Viewport を受け取り、幅を高さで割る関数を書くことです。長方形は `{ kind: "rect"; height: number; width: number }` の順で定義しました。二つのフィールドは同じ型で、縦長と横長では答えが異なります。引数に渡す JS オブジェクトのキー順も変えて検査しました。[課題](cases/aspect.md)・[入力](cases/aspect.ts)

| 支給した資料 | 初回コンパイルで全検査を通過 | 初見での判断 |
| --- | ---: | --- |
| 変更前の README | 2/2 | 2 人とも TS の定義を確認し、`Rect(height, width)` と書いた。タグだけの分岐とデータ付き分岐を一緒に書く例を、別の節と照合した。 |
| 型取り込み節の冒頭に `State` と `match` を並べ、宣言順を明記した版 | 2/2 | 2 人とも同じ節で括弧と順序を確認できた。どちらも、二つの値を実際に使う式があると分かりやすいと報告した。 |

コンパイラと課題は共通で、各回の実行役は履歴を引き継がずに起動しました。提出後に全件を別のディレクトリで再ビルドし、非表示、横長、縦長、小数、幅ゼロ、高さゼロと、公開関数の名前・型を確認しています。各人のコード、コンパイル履歴、自由記述、検証結果は [試行記録](rounds/aspect/) にあります。条件を伏せた別の採点者も、4 件すべてで機能・公開境界・利用経路を各 2 点と判定しました。[採点と並べ替え後の実測値](aspect-results.json)

## 宣言順だけを変えて再コンパイルする

提出物を固定し、TS の型定義を [width, height 順の版](cases/aspect-reordered.ts) に差し替えました。4 件ともコンパイルに成功し、横長・縦長・小数の検査が全件失敗しました。幅 12、高さ 3 では期待値が 4、実測が 0.25 です。JS オブジェクトのキー順だけを変えた元の課題は通っています。結果を左右するのは **TS の型宣言のフィールド順** です。

今回、初めて書く段階での取り違えは観測されませんでした。型定義の並べ替えで既存の計算が変わる問題は再現できました。README に二つの数値を使う `Rect(h, w)` の例と並べ替え時の注意を加えました。名前で束縛する構文を検討する際は、この並べ替え後にも同じテストを通せるかが比較基準になります。

## 再実行

`export fn` 対応版で `just build` を実行した後、次のコマンドで通常の検証と並べ替え後の検証を繰り返せます。2 行目は値の不一致で終了コード 1 を返します。

```sh
just replay rounds/aspect
just replay rounds/aspect cases/aspect-reordered.ts
```

支給資料はコミット `2ff5500a` の言語 README と、それに [試行時の差分](rounds/aspect/guide.patch) を適用した版です。試行後に追記した長方形の解答例は実行役には渡していません。追加の試行を行う場合は、支給する資料を明示してこの課題だけを選べます。

```sh
git show 2ff5500a:crates/zustscript/README.md > /tmp/zsc-union-original.md
patch -o /tmp/zsc-union-guide.md /tmp/zsc-union-original.md < rounds/aspect/guide.patch
just run /tmp/zsc-aspect-work /tmp/zsc-aspect-records /tmp/zsc-union-guide.md aspect
```
