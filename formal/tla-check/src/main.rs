//! Model-checks the TLA+ specs in `formal/tla` with idealis (rtla → rtla-compiler → rtlc).
//!
//! `cargo run --release -- <Spec.tla> [<Spec.cfg>] [--expect-violation]`
//! Exit status: 0 when the result is the expected one (complete and no violation, or a violation when
//! `--expect-violation` is given), 1 otherwise.

use rtlc::SafetyBounds;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let expect_violation = args.iter().any(|a| a == "--expect-violation");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let Some(tla) = files.first() else {
        eprintln!("usage: zust-tla-check <Spec.tla> [<Spec.cfg>] [--expect-violation]");
        std::process::exit(2);
    };
    let cfg = files.get(1).map(|s| s.to_string()).unwrap_or_else(|| tla.trim_end_matches(".tla").to_string() + ".cfg");
    let src = std::fs::read_to_string(tla).unwrap_or_else(|e| panic!("{tla}: {e}"));
    let cfg_src = std::fs::read_to_string(&cfg).unwrap_or_else(|e| panic!("{cfg}: {e}"));
    let module = rtla::parse_module(&src).unwrap_or_else(|e| panic!("{tla}: {e:?}"));
    let config = rtla::parse_config(&cfg_src).unwrap_or_else(|e| panic!("{cfg}: {e:?}"));
    let program = rtla_compiler::compile(&module, &config).unwrap_or_else(|e| panic!("{tla}: {e:?}"));
    let bounds = SafetyBounds { max_depth: 1_000, max_states: 5_000_000, max_work: 2_000_000_000, max_temporal_nodes: None };
    let t = std::time::Instant::now();
    let r = program.check(bounds).unwrap_or_else(|e| panic!("{tla}: {e:?}"));
    println!("{tla}: {} ({} states, depth {}, {:.2?})", r.status, r.states, r.depth, t.elapsed());
    if let Some(reason) = &r.reason {
        println!("  reason: {reason}");
    }
    if let Some(v) = &r.violation {
        println!("  violated: {}", v.name);
        for (i, step) in v.trace.iter().enumerate() {
            println!("  {i}: {}", step.action.as_deref().unwrap_or("Init"));
            for (name, val) in program.variables.iter().zip(&step.values) {
                println!("       {name} = {}", show(val));
            }
        }
    }
    let ok = if expect_violation { r.violation.is_some() } else { r.violation.is_none() && r.status == "complete" };
    std::process::exit(if ok { 0 } else { 1 });
}

fn show(v: &rtlc::Value) -> String {
    use rtlc::Value::*;
    let list = |xs: &[rtlc::Value]| xs.iter().map(show).collect::<Vec<_>>().join(", ");
    match v {
        Null => "null".into(),
        Bool(b) => if *b { "TRUE".into() } else { "FALSE".into() },
        Int(s) | Bytes(s) | ModelValue(s) => s.clone(),
        String(s) => format!("{s:?}"),
        List(xs) => format!("<<{}>>", list(xs)),
        Set(xs) => format!("{{{}}}", list(xs)),
        Record(fs) => format!("[{}]", fs.iter().map(|(k, v)| format!("{k} |-> {}", show(v))).collect::<Vec<_>>().join(", ")),
        Map(fs) => format!("({})", fs.iter().map(|(k, v)| format!("{} :> {}", show(k), show(v))).collect::<Vec<_>>().join(" @@ ")),
    }
}
