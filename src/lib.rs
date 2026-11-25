//! zustc: インクリメンタル開発ビルド特化の小さな Rust (サブセット) コンパイラ。
//! 中身は crates/ の crate に分かれている (このクレートは CLI と、それらの再輸出):
//! - `zust-base`: 部品 (FxHash の表・キャッシュのキー・直列化)
//! - `zust-syntax`: 字句・構文木・前処理 (cfg / マクロ / `mod` / 入れ子のアイテム)・アイテムの要約・prelude の選択
//! - `zust-sema`: Program (名前・型・トレイトの表)・型推論
//! - `zust-codegen`: acg (コード生成と型の検査)・Blob の直列化・リンク
//! - `zust-core`: session (キャッシュ・並列化・ワークスペース・check)。下の層を公開し直す
//! - `minclif`: Cranelift に似た API の小さな x86-64 バックエンド (外部 crate なし)
//! - `zust-clif`: session の Cranelift バックエンド (既定)
//! (base から core までは syn・Cranelift に依存せず、zustc 自身でもコンパイルできる)
//! LSP サーバ (zust-analyzer、エディタ向けの解析 zust-ide を使う) は別のバイナリで、このクレートは依存しない

pub use zust_core::*;
pub use zust_clif as clif_backend;
pub use zust_wasm as wasm_backend;
pub use zust_core::session::{Diagnostic, Options, BuildReport, Session};
