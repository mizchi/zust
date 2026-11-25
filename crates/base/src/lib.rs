//! zustc の全体で使う小さな部品 (外部クレートに依存しない。zustc 自身でもコンパイルできる)。
//! - fxhash: 名前表などの FxHasher / FxMap / FxSet
//! - key: キャッシュのキー (Key / KeyHasher)
//! - prof: 段ごとの時間の計測
//! - par: the front end's thread parallelism (an ordered parallel map on scoped threads; not self-hosted)

pub mod fxhash;
pub mod key;
pub mod par;
pub mod prof;
