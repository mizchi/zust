//! Exhaustive checks, over a small universe, of the lattice the borrow checker's dataflow runs on.
//!
//! The forward analysis in [`super::Bk::check`] is a worklist fixpoint: the state at a block's entry only grows by
//! [`super::State::join`], and the loop stops when no join reports a change. That is right only if
//! - the sorted sets stay sorted and without duplicates (`binary_search` relies on them): `add_sorted`, `join`;
//! - join gives exactly the union of both sides, and reports a change whenever the state (as sets) changed;
//! - joining what is already there changes nothing (so the loop can stop);
//! - the place order (`prefix`, `overlap`) is reflexive, transitive and antisymmetric / symmetric.
//!
//! Every input in the universe is tried (the small-scope hypothesis: such bugs show up on small inputs). `join`
//! treats its five parts independently (one loop each, the change flag is their or), so each part is enumerated on
//! its own against every value of the others' default. (These were first written as Kani harnesses; CBMC runs out
//! of memory on `Vec::insert` with a symbolic index even for sets of 4 elements, see formal/README.md.)

use super::super::SP;
use super::{BPlace, Moved, Proj, State, add_sorted, overlap, prefix};

/// All sorted sets (no duplicates) of values below n
fn all_sets(n: u32) -> Vec<Vec<u32>> {
    (0..1u32 << n).map(|b| (0..n).filter(|x| b >> x & 1 == 1).collect()).collect()
}

fn sorted_set(v: &[u32]) -> bool {
    v.windows(2).all(|w| w[0] < w[1])
}

fn bits(v: &[u32]) -> u32 {
    v.iter().fold(0, |b, x| b | 1 << x)
}

/// (local, loan) pairs `holds` says may be held, as bits (local * 8 + loan)
fn pairs(s: &State) -> u64 {
    s.holds.iter().fold(0, |b, (l, ls)| b | (bits(ls) as u64) << (l * 8))
}

fn moved_set(s: &State) -> u32 {
    s.moved.iter().fold(0, |b, m| b | 1 << (m.place.local * 2 + m.uninit as u32))
}

fn sets(s: &State) -> [u64; 5] {
    [bits(&s.inited) as u64, bits(&s.dead) as u64, bits(&s.escaped) as u64, pairs(s), moved_set(s) as u64]
}

fn well_formed(s: &State) -> bool {
    sorted_set(&s.inited) && sorted_set(&s.dead) && sorted_set(&s.escaped) && s.holds.iter().all(|h| sorted_set(&h.1))
}

/// Every value of `holds` over locals 0..2 and loans 0..3, in the orders the analysis can make: a local's entry
/// absent, empty, or any set, and the locals in either order
fn all_holds() -> Vec<Vec<(u32, Vec<u32>)>> {
    let mut opts: Vec<Option<Vec<u32>>> = vec![None];
    opts.extend(all_sets(3).into_iter().map(Some));
    let mut out = vec![];
    for a in &opts {
        for b in &opts {
            let mut h = vec![];
            if let Some(a) = a {
                h.push((0, a.clone()));
            }
            if let Some(b) = b {
                h.push((1, b.clone()));
            }
            if h.len() == 2 {
                out.push(vec![h[1].clone(), h[0].clone()]);
            }
            out.push(h);
        }
    }
    out
}

/// Every value of `moved` of up to two entries over locals 0..2, uninit or not (sp differs: it is not compared)
fn all_moved() -> Vec<Vec<Moved>> {
    let one: Vec<Moved> = (0..4).map(|i| Moved { place: BPlace { local: i / 2, proj: vec![] }, uninit: i % 2 == 1, sp: SP }).collect();
    let mut out = vec![vec![]];
    for a in 0..4 {
        out.push(vec![one[a].clone()]);
        for b in 0..4 {
            if a != b {
                out.push(vec![one[a].clone(), one[b].clone()]);
            }
        }
    }
    out
}

fn all_states() -> Vec<State> {
    let mut out = vec![];
    for s in all_sets(3) {
        out.push(State { inited: s.clone(), ..State::default() });
        out.push(State { dead: s.clone(), ..State::default() });
        out.push(State { escaped: s, ..State::default() });
    }
    for h in all_holds() {
        out.push(State { holds: h, ..State::default() });
    }
    for m in all_moved() {
        out.push(State { moved: m, ..State::default() });
    }
    out
}

#[test]
fn add_sorted_is_set_insert() {
    for v0 in all_sets(6) {
        for x in 0..6 {
            let mut v = v0.clone();
            let ch = add_sorted(&mut v, x);
            assert!(sorted_set(&v));
            assert_eq!(bits(&v), bits(&v0) | 1 << x);
            assert_eq!(ch, bits(&v0) >> x & 1 == 0);
        }
    }
}

#[test]
fn join_is_the_union_and_reports_changes() {
    let all = all_states();
    let mut n = 0;
    for a0 in &all {
        for b in &all {
            let mut a = a0.clone();
            let ch = a.join(b);
            assert!(well_formed(&a), "{a0:?} join {b:?} = {a:?}");
            let (sa, so, sb) = (sets(&a), sets(a0), sets(b));
            for i in 0..5 {
                assert_eq!(sa[i], so[i] | sb[i], "{a0:?} join {b:?} = {a:?}");
            }
            // (no change goes unreported)
            if !ch {
                assert_eq!(sa, so, "{a0:?} join {b:?}");
            }
            // (and the loop stops: joining b, or a itself, again reports no change)
            assert!(!a.join(b), "{a0:?} join {b:?} twice");
            let c = a.clone();
            assert!(!a.join(&c));
            n += 1;
        }
    }
    assert!(n > 10_000);
}

/// All paths of up to `max` projections
fn all_paths(max: usize) -> Vec<Vec<Proj>> {
    let alpha = [Proj::Field("a".into()), Proj::Field("b".into()), Proj::Deref(true), Proj::Deref(false), Proj::Shr, Proj::Index];
    let mut out = vec![vec![]];
    let mut last = vec![vec![]];
    for _ in 0..max {
        let mut next = vec![];
        for p in &last {
            for a in &alpha {
                let mut q: Vec<Proj> = p.clone();
                q.push(a.clone());
                next.push(q);
            }
        }
        out.extend(next.iter().cloned());
        last = next;
    }
    out
}

#[test]
fn place_order() {
    let ps = all_paths(2);
    for a in &ps {
        assert!(prefix(a, a));
        for b in &ps {
            if prefix(a, b) && prefix(b, a) {
                assert_eq!(a, b);
            }
            for c in &ps {
                if prefix(a, b) && prefix(b, c) {
                    assert!(prefix(a, c), "{a:?} {b:?} {c:?}");
                }
            }
            for (l1, l2) in [(0, 0), (0, 1)] {
                let (p, q) = (BPlace { local: l1, proj: a.clone() }, BPlace { local: l2, proj: b.clone() });
                assert_eq!(overlap(&p, &q), overlap(&q, &p));
                assert_eq!(overlap(&p, &q), l1 == l2 && (prefix(a, b) || prefix(b, a)));
            }
        }
    }
}
