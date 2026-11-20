//! zustc's prelude HashMap, compiled by zustc (drivers/hashmap.rs), follows formal/tla/HashMapModel.tla.
//! Needs `cargo build --release` at the repository root first (it runs target/release/zustc).

use idealis_connect::tla::{self, ConnectSpec, Driver, SafetyBounds, Status, Value};
use idealis_connect::{Generator, Plan, TestStatus};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A program zustc built, talked to one line per action
struct ZustProcess {
    _child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl ZustProcess {
    fn build(src: &str, name: &str) -> ZustProcess {
        let exe = std::env::temp_dir().join(format!("zust_connect_{name}_{}", std::process::id()));
        let st = Command::new(root().join("target/release/zustc")).arg(root().join(src)).arg("-o").arg(&exe).status().expect("target/release/zustc (cargo build --release at the root)");
        assert!(st.success(), "zustc {src}");
        let mut child = Command::new(&exe).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        ZustProcess { _child: child, input, output }
    }
    fn ask(&mut self, line: &str) -> String {
        writeln!(self.input, "{line}").unwrap();
        self.input.flush().unwrap();
        let mut s = String::new();
        self.output.read_line(&mut s).unwrap();
        s.trim_end().to_string()
    }
}

/// <<"insert", 1, 2>> -> "insert 1 2"; "prev 0" -> <<"prev", 0>>
fn line_of(action: &Value) -> String {
    let Value::List(xs) = action else { panic!("action {action:?}") };
    xs.iter()
        .map(|x| match x {
            Value::String(s) => s.clone(),
            Value::Int(n) => n.clone(),
            other => panic!("action part {other:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn value_of(reply: &str) -> Value {
    Value::List(
        reply
            .split_whitespace()
            .map(|w| if w.bytes().all(|b| b.is_ascii_digit()) { Value::Int(w.to_string()) } else { Value::String(w.to_string()) })
            .collect(),
    )
}

impl Driver for ZustProcess {
    fn reset(&mut self) -> Result<Value, String> {
        Ok(value_of(&self.ask("reset")))
    }
    fn step(&mut self, action: &Value) -> Result<Value, String> {
        Ok(value_of(&self.ask(&line_of(action))))
    }
}

const BOUNDS: SafetyBounds = SafetyBounds { max_depth: 10_000, max_states: 1_000_000, max_work: 2_000_000_000, max_temporal_nodes: None };

fn spec() -> (String, String) {
    let dir = root().join("formal/tla");
    (std::fs::read_to_string(dir.join("HashMapModel.tla")).unwrap(), std::fs::read_to_string(dir.join("HashMapModel.cfg")).unwrap())
}

/// The spec's state graph, keyed by every variable but `act` and `obs`
fn model() -> idealis_core::FiniteModel {
    let (tla, cfg) = spec();
    tla::finite_model(&tla, &cfg, ConnectSpec::default(), BOUNDS).unwrap()
}

/// 200 sequences of 1 to 100 steps, from a fixed seed; a failure is shrunk to a shortest sequence
fn check(model: &idealis_core::FiniteModel, driver: &mut impl Driver) -> tla::Report {
    let plan = Plan {
        generator: Generator::from_finite_model(model, "1", 1, 100).unwrap(),
        seed: 0x5eed,
        cases: 200,
        max_attempts: 800,
        max_shrink_checks: 512,
        max_work: 50_000_000,
    };
    tla::check(model, plan, driver).unwrap()
}

#[test]
fn prelude_hashmap_follows_the_model() {
    let model = model();
    assert_eq!(model.states.len(), 243);
    let mut sut = ZustProcess::build("formal/connect/drivers/hashmap.rs", "hashmap");
    let r = check(&model, &mut sut);
    assert!(r.counterexample.is_none(), "{:#?}", r.counterexample);
    assert_eq!(r.result.status, TestStatus::Passed, "{:?}", r.result);
}

/// The check finds a broken implementation and shrinks the failure (a map whose remove forgets to remove)
#[test]
fn a_broken_map_is_caught() {
    struct Broken(std::collections::HashMap<String, String>);
    impl Driver for Broken {
        fn reset(&mut self) -> Result<Value, String> {
            self.0.clear();
            Ok(value_of("ok"))
        }
        fn step(&mut self, action: &Value) -> Result<Value, String> {
            let l = line_of(action);
            let w: Vec<&str> = l.split(' ').collect();
            let prev = |m: &std::collections::HashMap<String, String>, k: &str| m.get(k).cloned().unwrap_or("0".into());
            Ok(match w[0] {
                "insert" => value_of(&format!("prev {}", self.0.insert(w[1].into(), w[2].into()).unwrap_or("0".into()))),
                // (the bug: the value stays)
                "remove" => value_of(&format!("prev {}", prev(&self.0, w[1]))),
                "get" => value_of(&format!("val {}", prev(&self.0, w[1]))),
                _ => value_of(&format!("len {}", self.0.len())),
            })
        }
    }
    let model = model();
    let r = check(&model, &mut Broken(Default::default()));
    assert_eq!(r.result.status, TestStatus::Failed);
    let actions: Vec<String> = r.result.counterexample.expect("the bug is found").actions.into_iter().map(|a| line_of(&serde_json::from_value(a).unwrap())).collect();
    let result = r.counterexample.expect("the confirmed mismatch");
    eprintln!("shrunk to {actions:?}: {:?} at step {}, expected one of {:?}", result.actual, result.checked_steps, result.expected);
    // (shrunk: insert, remove, then a step that sees the value still there)
    assert!(actions.len() <= 3, "{actions:?}");
    assert_eq!(result.status, Status::Mismatch);
}

/// CONSTRAINT / ACTION_CONSTRAINT prune the graph: a constraint that cuts the last key out leaves fewer states
#[test]
fn a_constrained_cfg_prunes_the_graph() {
    let (tla, cfg) = spec();
    let cut = tla.replace("====", "Small == m[5] = None\n====");
    let model = tla::finite_model(&cut, &format!("{cfg}\nCONSTRAINT Small\n"), ConnectSpec::default(), BOUNDS).unwrap();
    assert_eq!(model.states.len(), 81);
}
