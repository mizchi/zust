# 実験のレポート

zustc / zust-analyzer の作業の記録 (日付順)。数値はそれぞれの時点のもの。英語版は [../../reports/](../../reports/README.md)。

| 日付 | レポート |
|---|---|
| 2026-11-14 | [調査メモ: blaze / relax / wild](2026-11-14-01-blaze-relax-wild-survey.md) |
| 2026-11-15 | [生成したプログラムのインクリメンタルビルド (N = 1000 / 5000)](2026-11-15-01-incremental-bench.md) |
| 2026-11-15 | [rustc との比較（2026-11-15 時点。オーバーフロー検査・panic のメッセージなどを足した後）](2026-11-15-02-vs-rustc.md) |
| 2026-11-15 | [インクリメンタルビルドのディスク使用量](2026-11-15-03-incremental-disk-usage.md) |
| 2026-11-15 | [本物の rustc との比較（`python3 bench/vs_rustc.py`）](2026-11-15-04-vs-real-rustc.md) |
| 2026-11-15 | [strings と sort の `-O` を速くした時](2026-11-15-05-strings-sort-opt.md) |
| 2026-11-15 | [matmul と sieve の `-O`、ジェネリクスと単相化](2026-11-15-06-matmul-sieve-generics.md) |
| 2026-11-15 | [std の範囲へ（セルフホストの第 1 段階: zustc 自身の lexer）](2026-11-15-07-selfhost-01-lexer.md) |
| 2026-11-15 | [セルフホストの第 2 段階: enc.rs + fxhash.rs](2026-11-15-08-selfhost-02-enc-fxhash.md) |
| 2026-11-15 | [セルフホストの第 3 段階: prelude.rs](2026-11-15-09-selfhost-03-prelude.md) |
| 2026-11-15 | [セルフホストの第 4 段階: link.rs + testgen.rs](2026-11-15-10-selfhost-04-link-testgen.md) |
| 2026-11-15 | [セルフホストの第 5 段階: elfz.rs](2026-11-15-11-selfhost-05-elfz.md) |
| 2026-11-15 | [セルフホストの第 6 段階: macros.rs](2026-11-15-12-selfhost-06-macros.md) |
| 2026-11-15 | [セルフホストの第 7 段階: 自前の構文解析 (syn から離れる土台)](2026-11-15-13-selfhost-07-own-parser.md) |
| 2026-11-15 | [セルフホストの第 8 段階: クロージャ解析を自前の構文木へ移す](2026-11-15-14-selfhost-08-closure-analysis.md) |
| 2026-11-15 | [現状（2026-11-15、`python3 bench/vs_rustc.py`）](2026-11-15-15-status.md) |
| 2026-11-15 | [途中で分かったこと](2026-11-15-16-lessons.md) |
| 2026-11-15 | [回帰監視が見つけたもの](2026-11-15-17-regression-catches.md) |
| 2026-11-16 | [セルフホストの第 9 段階: front.rs の型の部分を分ける・モジュールの名前空間](2026-11-16-01-selfhost-09-front-types-modules.md) |
| 2026-11-16 | [セルフホストの第 10 段階: アイテムの要約を自前の構文木へ移す](2026-11-16-02-selfhost-10-item-summary.md) |
| 2026-11-16 | [セルフホストの第 11 段階: Program の表を syn から分ける](2026-11-16-03-selfhost-11-program-tables.md) |
| 2026-11-16 | [セルフホストの第 12 段階: 定数評価を自前の構文木へ移す](2026-11-16-04-selfhost-12-consteval.md) |
| 2026-11-16 | [セルフホストの第 13 段階: パスの検索を syn から分ける](2026-11-16-05-selfhost-13-path-lookup.md) |
| 2026-11-16 | [セルフホストの第 14 段階: 型推論の単一化器を分ける](2026-11-16-06-selfhost-14-unifier.md) |
| 2026-11-16 | [セルフホストの第 15 段階: 型推論を自前の構文木へ移す](2026-11-16-07-selfhost-15-type-inference.md) |
| 2026-11-16 | [セルフホストの第 16 段階: Cranelift の代わりのバックエンドの土台](2026-11-16-08-selfhost-16-backend-base.md) |
| 2026-11-16 | [セルフホストの第 17 段階: syn・Cranelift を使わない経路でプログラムを作る](2026-11-16-09-selfhost-17-no-syn-cranelift.md) |
| 2026-11-16 | [セルフホストの第 18 段階: codegen.rs を自前の構文木と minclif に移す (acg / driver2)](2026-11-16-10-selfhost-18-acg-driver2.md) |
| 2026-11-16 | [セルフホストの第 19 段階: zustc で zustc (syn・Cranelift を使わない経路) をコンパイルする](2026-11-16-11-selfhost-19-self-compile.md) |
| 2026-11-16 | [自前の構文木のソース位置](2026-11-16-12-ast-source-positions.md) |
| 2026-11-16 | [driver2 のキャッシュと並列化](2026-11-16-13-driver2-cache-parallel.md) |
| 2026-11-16 | [複数ファイル・複数クレート (`zustc build <workspace>`)](2026-11-16-14-multi-crate-build.md) |
| 2026-11-16 | [セルフホスト版のコンパイラの前半 (front) を速くする](2026-11-16-15-front-speedup.md) |
| 2026-11-16 | [小さい関数のインライン展開 (driver2 の `-O`)](2026-11-16-16-inlining.md) |
| 2026-11-16 | [minclif のレジスタ割り当て](2026-11-16-17-minclif-regalloc.md) |
| 2026-11-16 | [スタックの struct のフィールドをレジスタに置く (minclif)](2026-11-16-18-minclif-struct-fields-in-regs.md) |
| 2026-11-16 | [driver2 の Cranelift バックエンド (minclif と比べる)](2026-11-16-19-driver2-cranelift-backend.md) |
| 2026-11-16 | [最初のドライバ (syn + Cranelift) を外す](2026-11-16-20-remove-first-driver.md) |
| 2026-11-16 | [実在の小さなクレートをビルドする (`cratebench/`)](2026-11-16-21-cratebench.md) |
| 2026-11-16 | [依存の多い複数クレート (`bench/deps_bench.py`)](2026-11-16-22-deps-bench.md) |
| 2026-11-17 | [Cranelift の経路の関数のコンパイルを速くする (2026-11-17)](2026-11-17-01-cranelift-compile-speed.md) |
| 2026-11-17 | [Cranelift に渡す IR を小さくする (2026-11-17)](2026-11-17-02-cranelift-smaller-ir.md) |
| 2026-11-17 | [check を型検査で止めて LSP を速くする (2026-11-17)](2026-11-17-03-check-stops-at-typeck.md) |
| 2026-11-17 | [前処理をアイテムごとの差分にする (2026-11-17)](2026-11-17-04-incremental-prep.md) |
| 2026-11-17 | [check のキャッシュのキーと読み戻しを差分にする (2026-11-17)](2026-11-17-05-check-cache-diff.md) |
| 2026-11-17 | [LSP のシンボル検索 (2026-11-17)](2026-11-17-06-lsp-symbol-search.md) |
| 2026-11-17 | [LSP の比較: rust-analyzer / rust-glancer (2026-11-17、`python3 bench/lsp_compare.py --cold`)](2026-11-17-07-lsp-vs-rust-analyzer-glancer.md) |
| 2026-11-17 | [LSP の機能の比較 (2026-11-17、`python3 bench/lsp_parity.py`)](2026-11-17-08-lsp-parity.md) |
| 2026-11-17 | [LSP の定義ジャンプと参照 (2026-11-17)](2026-11-17-09-lsp-definition-references.md) |
| 2026-11-17 | [LSP の hover と補完、大きいワークスペースのメモリ (2026-11-17)](2026-11-17-10-lsp-hover-completion-memory.md) |
| 2026-11-17 | [LSP を zustc から分ける (2026-11-17)](2026-11-17-11-split-lsp.md) |
| 2026-11-17 | [references の残りの不一致を詰める (2026-11-17)](2026-11-17-12-references-mismatches.md) |
| 2026-11-17 | [ミューテーションテスト (cargo-mutants、2026-11-17)](2026-11-17-13-mutation-testing.md) |
| 2026-11-17 | [ide の見つからなかった変異を潰す (2026-11-17)](2026-11-17-14-ide-missed-mutants.md) |
| 2026-11-18 | [LSP に rename / signatureHelp / documentHighlight を足す (2026-11-18)](2026-11-18-01-lsp-rename-signature-highlight.md) |
| 2026-11-18 | [rust-analyzer との残りの違いを詰める: cfg・trait の impl の rename・signatureHelp (2026-11-18)](2026-11-18-02-ra-diffs-cfg-rename-signature.md) |
| 2026-11-18 | [LSP の比べ方を広げ、残りの違いを詰める (2026-11-18)](2026-11-18-03-lsp-parity-wider.md) |
| 2026-11-18 | [型推論の不足を詰める (2026-11-18)](2026-11-18-04-type-inference-gaps.md) |
| 2026-11-18 | [std のメソッドの参照の差を詰める (2026-11-18)](2026-11-18-05-std-method-references.md) |
| 2026-11-18 | [TODO.md の rust-analyzer との残りの違いを詰める](2026-11-18-06-remaining-differences.md) |
| 2026-11-18 | [rustc 自身の UI テストを仕様のテスト集にする](2026-11-18-07-rustc-ui-tests.md) |
| 2026-11-18 | [MIR なしの借用検査](2026-11-18-08-borrow-checking.md) |
| 2026-11-18 | [KPI を見ながら tests/ui の互換性を上げる](2026-11-18-09-compat-with-kpis.md) |
| 2026-11-19 | [形式モデルと有界な証明: 借用検査・スレッドモデル・アロケータ](2026-11-19-01-formal-models.md) |
| 2026-11-20 | [形式モデルが見つけた欠陥の修正: 借用検査の打ち切り・Vec の桁あふれと割り当て失敗・キャッシュの一時ファイル](2026-11-20-01-formal-fixes.md) |
| 2026-11-20 | [`std::thread` をグリーンスレッドに](2026-11-20-02-green-threads.md) |
| 2026-11-20 | [アプリケーションのコードの形のベンチマーク](2026-11-20-03-bench-patterns.md) |
| 2026-11-21 | [Rust の CVE から作るセキュリティの回帰テスト、同じパターンの点検、wasm のサンドボックス](2026-11-21-01-security-cves.md) |
| 2026-11-21 | [RefCell の借用フラグとガード、一時値を drop する時](2026-11-21-02-refcell-guards.md) |
| 2026-11-21 | [zustc で dioxus と leptos を wasm32 向けにビルドできるか](2026-11-21-03-ui-frameworks-wasm.md) |
| 2026-11-21 | [zurgo M0: Cargo のパッケージを zustc でビルドする](2026-11-21-04-zurgo-m0.md) |
| 2026-11-20 | [リージョン (ライフタイム) 検査の方針と手順 (issue #1)](2026-11-20-01-region-check.md) |
| 2026-11-21 | [リージョン (ライフタイム) 検査、その 2: 最初の版が残したもの](2026-11-21-05-region-check-2.md) |
| 2026-11-21 | [リージョン (ライフタイム) 検査、その 3: クロージャ、関数より長く生きる借用、lint](2026-11-21-06-region-check-3.md) |
| 2026-11-21 | [リージョン (ライフタイム) 検査、その 4: 何も呼ばないジェネリック関数](2026-11-21-07-region-check-4.md) |
| 2026-11-22 | [リージョン (ライフタイム) 検査、その 5: 型パラメータのリージョン](2026-11-22-01-region-check-5.md) |
| 2026-11-22 | [リージョン (ライフタイム) 検査、その 6: どこでも同じ代わりの型のリージョンと、借用検査の可変性](2026-11-22-02-region-check-6.md) |
| 2026-11-22 | [リージョン (ライフタイム) 検査、その 7: 制御フロー、一時値、prelude のメソッドが返すもの](2026-11-22-03-region-check-7.md) |
| 2026-11-22 | [zurgo M1: ビルドスクリプト、`env!`、proc-macro のスタブ](2026-11-22-04-zurgo-build-scripts.md) |
| 2026-11-22 | [リージョン (ライフタイム) 検査、その 8: `&mut` を通した書き込み、ムーブできないものからのムーブ、チューニング](2026-11-22-05-region-check-8.md) |
| 2026-11-22 | [リージョン (ライフタイム) 検査、その 9: ブロックの末尾、match のガード、デストラクタ、関数本体の中のアイテム](2026-11-22-06-region-check-9.md) |
| 2026-11-22 | [リージョン (ライフタイム) 検査、その 10: クロージャのキャプチャ、ガード、パターン、暗黙の再借用、`'static` のインスタンス](2026-11-22-07-region-check-10.md) |
| 2026-11-22 | [W0: wasm-bindgen-shared を zurgo で wasm32 向けに](2026-11-22-08-wasm-bindgen-shared-wasm32.md) |
| 2026-11-22 | [リージョン (ライフタイム) 検査、その 11: match の末尾、末尾の一時値、クロージャの本体、パスに書いたライフタイム、トレイトオブジェクトの受け手](2026-11-22-08-region-check-11.md) |
| 2026-11-22 | [リージョン (ライフタイム) 検査、その 12: 共有の deref、クロージャの逃げ出しと再借用、注釈のあるパターン、impl のシグネチャ](2026-11-22-09-region-check-12.md) |
| 2026-11-23 | [リージョン (ライフタイム) 検査、その 13: impl のヘッダ、`&mut` のムーブと再借用、二相借用、クロージャの `'static` 注釈、mismatched_lifetime_syntaxes リント](2026-11-23-01-region-check-13.md) |
| 2026-11-23 | [リージョン (ライフタイム) 検査、その 14: `impl Trait` の結果のキャプチャ、クロージャの `T: 'static`、ローカルを通した二相借用、`ref` の残りのパターン](2026-11-23-02-region-check-14.md) |
| 2026-11-23 | [リージョン (ライフタイム) 検査、その 15: スライスのパターン、クロージャの変数全体のキャプチャ、`copied` のイテレータ、エディション 2024 のブロックの末尾、`unused_mut`、GAT の引数](2026-11-23-03-region-check-15.md) |
| 2026-11-23 | [プログラムのトレイトの trait solver: 先に Lean でモデル化・証明してから実装する](2026-11-23-04-trait-solver.md) |
| 2026-11-23 | [trait solver、その 2: コヒーレンス、スーパートレイト、dyn への型強制、impl の選択、auto trait](2026-11-23-05-trait-solver-2.md) |
| 2026-11-23 | [trait solver、その 3: 関連型](2026-11-23-06-trait-solver-3.md) |
| 2026-11-23 | [trait solver、その 4: 誤ったエラーを減らす](2026-11-23-07-trait-solver-4.md) |
