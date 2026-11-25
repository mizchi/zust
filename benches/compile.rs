//! Benchmarks for regression monitoring (criterion).
//!
//! - lsp/*   : LSP responsiveness. The time from didChange to publishDiagnostics (in-process)
//! - build/* : build speed. cold / no-op / editing 1 fn (including the link)
//! - front/* : lexing only (the lower bound for no-op)
//!
//! See bench/regress.sh for usage (saving a baseline and comparing).
//! The linker can be pinned with the environment variable ZUSTC_BENCH_LINKER (chosen automatically if unset).

use std::path::PathBuf;
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use zust_analyzer::Server;
use zustc::testgen::program;
use zustc::{Options2, Session2};
use serde_json::json;

const N: usize = 1000;

fn opts() -> Options2 {
    Options2 {
        linker: std::env::var("ZUSTC_BENCH_LINKER").ok(),
        backend: Some(zustc::clif_backend::compile),
        ..Options2::default()
    }
}

fn open_msg(text: &str) -> String {
    json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": "file:///b.rs", "languageId": "rust", "version": 1, "text": text}}})
    .to_string()
}

fn change_msg(text: &str) -> String {
    json!({"jsonrpc": "2.0", "method": "textDocument/didChange",
        "params": {"textDocument": {"uri": "file:///b.rs", "version": 2}, "contentChanges": [{"text": text}]}})
    .to_string()
}

fn tmpdir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bench");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn lsp(c: &mut Criterion) {
    let v = [program(N, 0), program(N, 1), program(N, 2)];
    let (c1, c2) = (change_msg(&v[1]), change_msg(&v[2]));
    let mut g = c.benchmark_group("lsp");

    // A didChange that edits just 1 fn (alternating between edits, so 1 fn is rechecked each time)
    let mut srv = Server::new();
    srv.handle(&open_msg(&v[0]));
    let mut k = false;
    g.bench_function(format!("did_change_1fn/{N}"), |b| {
        b.iter(|| {
            k = !k;
            srv.handle(if k { &c1 } else { &c2 })
        })
    });

    // A didChange whose content does not change (just a save, etc.)
    let same = change_msg(&v[0]);
    let mut srv = Server::new();
    srv.handle(&open_msg(&v[0]));
    g.bench_function(format!("did_change_noop/{N}"), |b| {
        b.iter(|| srv.handle(&same))
    });

    g.finish();
}

/// The cold ones vary by about ±10% per run due to thread scheduling and the page cache, so
/// they are put in a separate group with a regression threshold of 15%
fn cold(c: &mut Criterion) {
    let v = program(N, 0);
    let mut g = c.benchmark_group("cold");
    g.noise_threshold(0.15);
    // LSP: the first open (checks all fns)
    let open = open_msg(&v);
    g.bench_function(format!("lsp_did_open/{N}"), |b| {
        b.iter_batched(
            Server::new,
            |mut s| s.handle(&open),
            BatchSize::PerIteration,
        )
    });
    let out = tmpdir().join("cold");
    g.bench_function(format!("build/{N}"), |b| {
        b.iter_batched(
            || Session2::new(opts()),
            |mut s: Session2| s.build(&v, &out).unwrap(),
            BatchSize::PerIteration,
        )
    });
    g.finish();
}

fn build(c: &mut Criterion) {
    let v = [program(N, 0), program(N, 1), program(N, 2)];
    let dir = tmpdir();
    let mut g = c.benchmark_group("build");

    // The same flow as the CLI: build → save_cache (keeps only the entries used this time)
    let out = dir.join("noop");
    let cp = out.with_extension("crcache2");
    let mut s = Session2::new(opts());
    s.build(&v[0], &out).unwrap();
    s.save_cache(&cp).unwrap();
    g.bench_function(format!("noop/{N}"), |b| {
        b.iter(|| {
            let r = s.build(&v[0], &out).unwrap();
            s.save_cache(&cp).unwrap();
            assert!(!r.linked);
        })
    });

    let out = dir.join("edit");
    let cp = out.with_extension("crcache2");
    let mut s = Session2::new(opts());
    s.build(&v[1], &out).unwrap();
    s.save_cache(&cp).unwrap();
    let mut k = false;
    g.bench_function(format!("edit_1fn/{N}"), |b| {
        b.iter(|| {
            k = !k;
            let r = s.build(if k { &v[2] } else { &v[1] }, &out).unwrap();
            s.save_cache(&cp).unwrap();
            assert_eq!(r.fns_compiled, 1);
        })
    });
    g.finish();
}

fn front(c: &mut Criterion) {
    let v = program(N, 0);
    c.bench_function(&format!("front/lex/{N}"), |b| {
        b.iter(|| zustc::lexer::split(&v).unwrap().len())
    });
}

fn config() -> Criterion {
    // Ignore changes under 5%, so that noise is not treated as a regression
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(3))
        .noise_threshold(0.05)
        .significance_level(0.01)
}

criterion_group! { name = benches; config = config(); targets = lsp, build, cold, front }
criterion_main!(benches);
