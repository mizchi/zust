//! Connect a TLA+ model to an implementation, in the way of Quint Connect, with idealis:
//!
//! 1. [`finite_model`] model-checks the spec (rtla / rtla-compiler / rtlc) and turns its state graph into an idealis
//!    `FiniteModel`. The spec names each step in a variable `act` (the action, with its arguments) and what the step
//!    observes in `obs`; the state the implementation is compared on is every other variable. Init sets `act` to
//!    `<<"reset">>`.
//! 2. [`check`] has idealis-testing generate action sequences from that graph (a property session: random walks,
//!    shrinking a failure to a shortest sequence), and runs each one through idealis-core's conformance `Session`:
//!    the model answers which observations it allows after each action, a [`Driver`] runs the action on the
//!    implementation and returns what it observed. A step whose observation the model does not allow is a mismatch.
//!
//! The model's observations come from the spec, never from the implementation.

use idealis_core::{ConformanceResult, Edge, Effect, FiniteModel, FiniteState, Initial, Response, Session, Status};
use rtlc::{SafetyBounds, SafetyExpansionCache, Value};
use std::collections::BTreeMap;

/// The implementation side: `reset` makes a new one and returns its initial observation; `step` runs one action
/// (the spec's `act` value) and returns what it observed (to compare with the spec's `obs`)
pub trait Driver {
    fn reset(&mut self) -> Value;
    fn step(&mut self, action: &Value) -> Value;
}

/// The spec's state graph as an idealis FiniteModel (states keyed by every variable but `act` and `obs`)
pub fn finite_model(name: &str, tla: &str, cfg: &str) -> Result<FiniteModel, String> {
    let module = rtla::parse_module(tla).map_err(|e| format!("{e:?}"))?;
    let config = rtla::parse_config(cfg).map_err(|e| format!("{e:?}"))?;
    let program = rtla_compiler::compile(&module, &config).map_err(|e| format!("{e:?}"))?;
    let vars = program.variables.clone();
    let ia = vars.iter().position(|v| v == "act").ok_or("the spec has no variable `act`")?;
    let io = vars.iter().position(|v| v == "obs").ok_or("the spec has no variable `obs`")?;
    // (the expansion cache keeps every state's successors: the state graph)
    let mut cache = SafetyExpansionCache::default();
    let opts = rtlc::ExpansionOptions { enabled: true, build_digest: &"0".repeat(64), max_entries: 1_000_000 };
    let bounds = SafetyBounds { max_depth: 10_000, max_states: 1_000_000, max_work: 2_000_000_000, max_temporal_nodes: None };
    let r = program.check_incremental(bounds, &mut cache, opts).map_err(|e| format!("{e:?}"))?;
    if r.result.status != "complete" {
        return Err(format!("model checking: {} ({:?})", r.result.status, r.result.reason));
    }
    let snap = serde_json::to_value(&cache).map_err(|e| e.to_string())?;
    let entries = snap["entries"].as_object().ok_or("no entries in the expansion cache")?;
    let project = |vals: &[Value]| -> String {
        let rest: Vec<&Value> = vals.iter().enumerate().filter(|(i, _)| *i != ia && *i != io).map(|(_, v)| v).collect();
        serde_json::to_string(&rest).unwrap()
    };
    let mut states: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
    let mut initial: Vec<Initial> = vec![];
    for (key, e) in entries {
        let vals: Vec<Value> = serde_json::from_str(key).map_err(|e| format!("state key: {e}"))?;
        let from = project(&vals);
        if vals[ia] == Value::List(vec![Value::String("reset".into())]) && !initial.iter().any(|i| i.key == from) {
            initial.push(Initial { key: from.clone(), observation: vals[io].clone() });
        }
        let edges = states.entry(from).or_default();
        for s in e["successors"].as_array().ok_or("no successors")? {
            let next: Vec<Value> = serde_json::from_value(s["values"].clone()).map_err(|e| format!("successor: {e}"))?;
            let edge = Edge { action: next[ia].clone(), target: project(&next), observation: next[io].clone() };
            if !edges.iter().any(|x| x.action == edge.action && x.target == edge.target && x.observation == edge.observation) {
                edges.push(edge);
            }
        }
    }
    if initial.is_empty() {
        return Err("no initial state (Init must set act to <<\"reset\">>)".into());
    }
    let states = states.into_iter().map(|(key, transitions)| FiniteState { key, violations: vec![], transitions }).collect();
    Ok(FiniteModel { format: "modelt.finite-model".into(), version: 1, name: name.into(), initial, states })
}

/// Run `actions` on the model and on the driver, step by step (idealis-core's conformance session)
pub fn conform(model: &FiniteModel, actions: Vec<Value>, driver: &mut dyn Driver) -> Result<ConformanceResult, String> {
    let e = |e: rtlc::CoreError| format!("{e:?}");
    let mut s = Session::new(model.name.clone(), actions).map_err(e)?;
    loop {
        let effect = s.effect();
        if let Some(response) = model.response(&effect).map_err(e)? {
            let (Effect::ModelInitial { id } | Effect::ModelStep { id, .. }) = effect else { unreachable!() };
            s.resume(id, response).map_err(e)?;
            continue;
        }
        match effect {
            Effect::Done { result } => return Ok(result),
            Effect::DriverReset { id } => {
                s.resume(id, Response::Observation { observation: driver.reset() }).map_err(e)?;
            }
            Effect::DriverStep { id, action } => {
                let observation = driver.step(&action);
                s.resume(id, Response::Observation { observation }).map_err(e)?;
            }
            _ => unreachable!(),
        }
    }
}

/// What [`check`] found
#[derive(Debug)]
pub struct Report {
    pub status: idealis_testing::TestStatus,
    pub cases: u32,
    /// A shortest action sequence the implementation does not follow, with the step where it differs
    pub counterexample: Option<(Vec<Value>, ConformanceResult)>,
}

/// Generate `cases` action sequences of at most `max_length` steps from the model's graph (seed: any nonzero) and
/// check that the driver follows the model on each; a failure is shrunk to a shortest sequence
pub fn check(model: &FiniteModel, driver: &mut dyn Driver, seed: u32, cases: u32, max_length: usize) -> Result<Report, String> {
    use idealis_testing::{ActionEdge, Effect as TEffect, Generator, Plan, Session as TSession, Verdict};
    let to_json = |v: &Value| serde_json::to_value(v).unwrap();
    let generator = Generator {
        id: model.name.clone(),
        version: "1".into(),
        initial: model.initial.iter().map(|i| i.key.clone()).collect(),
        states: model
            .states
            .iter()
            .map(|s| (s.key.clone(), s.transitions.iter().map(|t| ActionEdge { action: to_json(&t.action), target: t.target.clone() }).collect()))
            .collect(),
        min_length: 1,
        max_length,
    };
    let plan = Plan { generator, seed, cases, max_attempts: cases * 4, max_shrink_checks: 512, max_work: 50_000_000 };
    let mut session = TSession::property(plan).map_err(|e| format!("{e:?}"))?;
    let run = |actions: &[serde_json::Value], driver: &mut dyn Driver| -> Result<(Vec<Value>, ConformanceResult), String> {
        let acts: Vec<Value> = actions.iter().map(|a| serde_json::from_value(a.clone()).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
        let r = conform(model, acts.clone(), driver)?;
        Ok((acts, r))
    };
    loop {
        match session.effect() {
            TEffect::Evaluate { id, case, .. } => {
                let verdict = match run(&case.actions, driver) {
                    Ok((_, r)) if r.status == Status::Matched => Verdict::Matched {},
                    // (one signature for every mismatch: shrinking may then cut the sequence down to any shorter one
                    // that still fails)
                    Ok((_, r)) => Verdict::Mismatch { signature: format!("{:?}", r.status) },
                    Err(e) => Verdict::Error { message: e },
                };
                session.resume(id, verdict).map_err(|e| format!("{e:?}"))?;
            }
            TEffect::Done { result } => {
                let counterexample = match &result.counterexample {
                    Some(case) => Some(run(&case.actions, driver)?),
                    None => None,
                };
                return Ok(Report { status: result.status, cases: result.cases, counterexample });
            }
        }
    }
}
