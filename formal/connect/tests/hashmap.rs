//! zustc's prelude HashMap, compiled by zustc (drivers/hashmap.rs), follows formal/tla/HashMapModel.tla.
//! Needs `cargo build --release` at the repository root first (it runs target/release/zustc).

use rtlc::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use zust_connect::{Driver, check, finite_model};

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
    fn reset(&mut self) -> Value {
        value_of(&self.ask("reset"))
    }
    fn step(&mut self, action: &Value) -> Value {
        value_of(&self.ask(&line_of(action)))
    }
}

fn model() -> idealis_core::FiniteModel {
    let tla = std::fs::read_to_string(root().join("formal/tla/HashMapModel.tla")).unwrap();
    let cfg = std::fs::read_to_string(root().join("formal/tla/HashMapModel.cfg")).unwrap();
    finite_model("HashMapModel", &tla, &cfg).unwrap()
}

#[test]
fn prelude_hashmap_follows_the_model() {
    let model = model();
    assert_eq!(model.states.len(), 243);
    let mut sut = ZustProcess::build("formal/connect/drivers/hashmap.rs", "hashmap");
    let r = check(&model, &mut sut, 0x5eed, 200, 100).unwrap();
    assert!(r.counterexample.is_none(), "{:#?}", r.counterexample);
    assert_eq!(r.status, idealis_testing::TestStatus::Passed, "{r:?}");
}

/// The check finds a broken implementation and shrinks the failure (a map whose remove forgets to remove)
#[test]
fn a_broken_map_is_caught() {
    struct Broken(std::collections::HashMap<String, String>);
    impl Driver for Broken {
        fn reset(&mut self) -> Value {
            self.0.clear();
            value_of("ok")
        }
        fn step(&mut self, action: &Value) -> Value {
            let l = line_of(action);
            let w: Vec<&str> = l.split(' ').collect();
            let prev = |m: &std::collections::HashMap<String, String>, k: &str| m.get(k).cloned().unwrap_or("0".into());
            match w[0] {
                "insert" => value_of(&format!("prev {}", self.0.insert(w[1].into(), w[2].into()).unwrap_or("0".into()))),
                // (the bug: the value stays)
                "remove" => value_of(&format!("prev {}", prev(&self.0, w[1]))),
                "get" => value_of(&format!("val {}", prev(&self.0, w[1]))),
                _ => value_of(&format!("len {}", self.0.len())),
            }
        }
    }
    let model = model();
    let r = check(&model, &mut Broken(Default::default()), 0x5eed, 200, 100).unwrap();
    let (actions, result) = r.counterexample.expect("the bug is found");
    eprintln!("shrunk to {actions:?}: {:?} at step {}, expected one of {:?}", result.actual, result.checked_steps, result.expected);
    assert_eq!(r.status, idealis_testing::TestStatus::Failed);
    // (shrunk: insert, remove, then a step that sees the value still there)
    assert!(actions.len() <= 3, "{actions:?}");
    assert_eq!(result.status, idealis_core::Status::Mismatch);
}

#[test]
fn a_constrained_cfg_is_rejected() {
    // (the state graph is read before CONSTRAINT / ACTION_CONSTRAINT prune it)
    let dir = root().join("formal/tla");
    let tla = std::fs::read_to_string(dir.join("HashMapModel.tla")).unwrap();
    let cfg = std::fs::read_to_string(dir.join("HashMapModel.cfg")).unwrap();
    for extra in ["CONSTRAINT TypeOK", "ACTION_CONSTRAINT TypeOK"] {
        let err = finite_model("HashMapModel", &tla, &format!("{cfg}\n{extra}\n")).unwrap_err();
        assert!(err.contains("CONSTRAINT"), "{err}");
    }
}
