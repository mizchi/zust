//! The library side of zust-fuzz: the mutations, the targets, and running one input with its panic caught. The
//! binary (main.rs) is the fuzz loop; tests/regressions.rs replays the findings kept in tests/fuzz/.

pub mod mutate;
pub mod rng;
pub mod targets;

use std::sync::Mutex;
use targets::{Outcome, Target};

/// What went wrong with one input, if anything: the kind and the place (a panic's location), which together tell
/// one bug from another.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Failure {
    pub kind: &'static str,
    pub place: String,
    pub detail: String,
}

static PANIC: Mutex<Option<(String, String)>> = Mutex::new(None);

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let place = info.location().map_or_else(|| "?".to_string(), |l| format!("{}:{}", l.file(), l.line()));
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            String::new()
        };
        if let Ok(mut p) = PANIC.lock() {
            *p = Some((place, msg));
        }
    }));
}

/// Runs one input through one target: a panic or a mismatch is a failure, anything else the answer's feature.
pub fn run_one(t: Target, src: &str, edition: u16) -> Result<String, Failure> {
    match std::panic::catch_unwind(|| targets::run(t, src, edition)) {
        Ok(Outcome::Done(f)) => Ok(f),
        Ok(Outcome::Mismatch(d)) => Err(Failure { kind: "mismatch", place: t.name().to_string(), detail: d }),
        Err(_) => {
            let (place, msg) = PANIC.lock().ok().and_then(|mut p| p.take()).unwrap_or_default();
            Err(Failure { kind: "panic", place: format!("{}@{place}", t.name()), detail: msg })
        }
    }
}

/// A stable name for a finding's file (FNV-1a).
pub fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}
