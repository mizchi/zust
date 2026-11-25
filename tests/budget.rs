//! Size budget test: fails if the binaries, objects or cache (driver2 + Cranelift) exceed the budget (bench/budget.txt).
//! When they were increased on purpose, rewrite the budget with `UPDATE_BUDGET=1 cargo test --release --test budget`
//! (it is written as the current values + a 5% margin). If you only added new examples, `UPDATE_BUDGET=new` adds them without changing the existing limits.

use std::collections::BTreeMap;
use std::path::PathBuf;

use zustc::{Options2, Session2, testgen::program};

const BUDGET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/bench/budget.txt");

fn measure() -> BTreeMap<String, u64> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("budget");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = BTreeMap::new();
    let mut cases: Vec<(String, String)> = vec![("gen1000".into(), program(1000, 0))];
    for e in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/examples")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            cases.push((name, std::fs::read_to_string(&p).unwrap()));
        }
    }
    for (name, src) in &cases {
        let out = dir.join(name);
        // The size changes with the linker, so use the default (automatic choice), not bfd. In CI it is fixed with ZUSTC_BENCH_LINKER
        let o = Options2 {
            linker: std::env::var("ZUSTC_BENCH_LINKER").ok(),
            backend: Some(zustc::clif_backend::compile),
            ..Options2::default()
        };
        let mut s = Session2::new(o);
        let r = s.build(src, &out).unwrap_or_else(|e| panic!("{name}: {e}"));
        let cache = s.save_cache(&dir.join(format!("{name}.crcache2"))).unwrap();
        m.insert(format!("{name}.binary_bytes"), std::fs::metadata(&out).unwrap().len());
        m.insert(format!("{name}.object_bytes"), r.object_bytes as u64);
        m.insert(format!("{name}.cache_bytes"), cache as u64);
    }
    m
}

#[test]
fn sizes_within_budget() {
    let now = measure();
    if let Some(mode) = std::env::var_os("UPDATE_BUDGET") {
        // With UPDATE_BUDGET=new, the existing limits stay as they are and only the new entries are added
        let old: BTreeMap<String, u64> = std::fs::read_to_string(BUDGET)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                l.split_once(' ')
                    .map(|(k, v)| (k.to_string(), v.trim().parse().unwrap()))
            })
            .collect();
        let keep_old = mode == "new";
        let body: String = now
            .iter()
            .map(|(k, v)| match old.get(k) {
                Some(o) if keep_old => format!("{k} {o}\n"),
                _ => format!("{k} {}\n", v + v / 20),
            })
            .collect();
        std::fs::write(
            BUDGET,
            format!("# 名前 上限バイト (UPDATE_BUDGET=1 で再生成: 現在値 + 5%)\n{body}"),
        )
        .unwrap();
        return;
    }
    let text =
        std::fs::read_to_string(BUDGET).expect("bench/budget.txt がない: UPDATE_BUDGET=1 で作る");
    let mut over = vec![];
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let (k, v) = line.split_once(' ').unwrap();
        let limit: u64 = v.trim().parse().unwrap();
        match now.get(k) {
            Some(&cur) if cur > limit => over.push(format!("{k}: {cur} > {limit}")),
            Some(_) => {}
            None => over.push(format!("{k}: 計測されていない")),
        }
    }
    for (k, v) in &now {
        eprintln!("{k:<28} {v:>10}");
    }
    assert!(over.is_empty(), "サイズ予算超過:\n{}", over.join("\n"));
}
