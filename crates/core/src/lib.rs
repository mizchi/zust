//! zustc のドライバ。session (Session) が解析・check (LSP の診断)・キャッシュと、feature `build` (既定) のときは
//! ワークスペースのビルド (並列化・機械語・リンク) をまとめる。
//! 層: base → syntax → sema → typeck (型の検査・命令の記録) → codegen (機械語・リンク: build のときだけ) → core (ここ)。
//! LSP は zust-analyzer (build なしで使う)、Cranelift のバックエンドは zust-clif。下の層のモジュールはここからも
//! `zust_core::lexer` のように引ける

pub use zust_typeck::*;
#[cfg(feature = "build")]
pub use zust_codegen::{link, lower, minclif};

pub mod session;
#[doc(hidden)]
pub mod testgen;
