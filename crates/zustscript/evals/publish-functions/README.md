# 公開関数を書く試行

初めて zustscript を使うエージェントにモジュールを一つ書いてもらい、生成された JS をアプリから呼べるところまで確認します。配送ラベルでは TypeScript の型と関数の公開範囲を、波形の二乗和では Wasm と JS の呼び分けを扱います。[初回の結果とフィードバック](report.md)・[union のフィールド順の追加評価](aspect-report.md)

## 一周させる

`export fn` 対応版（zust のコミット `2ff5500a` 以降）のチェックアウトで、このディレクトリから実行します。Rust、Node.js 24 以降、Python 3.9 以降、just、認証済みの Codex CLI を使います。次のコマンドは各課題を独立した新規セッションで 2 回ずつ実行します。エージェントのログは作業ディレクトリに残ります。実行には Codex の利用枠を使います。

```sh
just build
just run /tmp/zsc-work-01 /tmp/zsc-records-01
just replay /tmp/zsc-records-01
```

評価ファイルを置いたブランチが `export fn` 導入前の場合も、コンパイラと支給資料のチェックアウトを指定できます。収録した提出物を手元で再検証する例です。

```sh
git worktree add --detach /tmp/zsc-toolchain 2ff5500a
just --set toolchain /tmp/zsc-toolchain build
just --set toolchain /tmp/zsc-toolchain replay rounds/01
just --set toolchain /tmp/zsc-toolchain replay rounds/02
```

実行役は [共通の指示](task.md)、自分の課題、指定した版の言語 README、コンパイラだけを渡されます。コンパイルするたびにソース、診断、終了コードが保存されます。回収時には [検証器](verify.mjs) が別の一時ディレクトリで提出物を再ビルドし、JS からの実行結果、型定義、公開名、Wasm のエクスポートを調べます。数値採点は [判定基準](criteria.md) に従い、実行役と別の担当者が行います。

改善案を試すときは README のコピーを編集し、3 番目の引数に渡します。作業場所と結果の保存先を毎回新しくし、課題、コンパイラ、モデル、試行回数をそろえます。今回の比較用資料は次の手順で復元できます。

```sh
git show 2ff5500a:crates/zustscript/README.md > /tmp/zsc-guide-before.md
patch -o /tmp/zsc-guide-after.md /tmp/zsc-guide-before.md < guide.patch
just run /tmp/zsc-work-02 /tmp/zsc-records-02 /tmp/zsc-guide-after.md
```

## 対話セッションの subagent で実行する

この方式で [収録した2ラウンド](rounds/) を実施しました。コントローラーが `just stage status /tmp/zsc-status-01`（課題名は `status`、`energy`、`aspect`）で試行用ディレクトリを用意し、履歴を引き継がない新しい subagent に次の指示だけを渡します。

> 指定ディレクトリの TASK.md と CASE.md に従って実際に作業してください。作業ファイルと資料はそのディレクトリ内だけを参照してください。

各試行の終了後に `just collect status /tmp/zsc-status-01 /tmp/zsc-records/status-1.json` で回収します。別の README を使うときは `stage.py --guide` を指定します。採点担当者には条件名を伏せ、課題、判定基準、提出ソース、コンパイル履歴、検証結果を渡してください。本人の感想と独立検証の結果は分けて集計します。資料の参照範囲は指示で制限しており、ファイルシステムでの隔離を保証する方式ではありません。
