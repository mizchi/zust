//! Proofs (bounded) about the prelude's `Vec<T>`.

use super::Vec;
use super::alloc_model::{ALLOC_MAY_FAIL, LIVE};

const MAXLEN: usize = 4;

/// The reference: a fixed array and a length
struct Model {
    a: [u8; MAXLEN],
    n: usize,
}

fn same(v: &Vec<u8>, m: &Model) {
    assert_eq!(v.len(), m.n);
    assert!(v.len() <= v.capacity());
    for i in 0..MAXLEN {
        if i < m.n {
            assert_eq!(*v.get(i).unwrap(), m.a[i]);
        } else {
            assert!(v.get(i).is_none());
        }
    }
}

/// A Vec in any state reachable with up to maxk (< MAXLEN) pushes after `with_capacity(c)`, c <= MAXLEN, and the
/// reference holding the same values
fn build(maxk: usize) -> (Vec<u8>, Model) {
    let c = (kani::any::<u8>() as usize) % (MAXLEN + 1);
    let k = (kani::any::<u8>() as usize) % (maxk + 1);
    let mut v: Vec<u8> = Vec::with_capacity(c);
    let mut m = Model { a: [0; MAXLEN], n: 0 };
    for j in 0..MAXLEN - 1 {
        if j < k {
            let x: u8 = kani::any();
            v.push(x);
            m.a[j] = x;
            m.n += 1;
        }
    }
    same(&v, &m);
    (v, m)
}

/// Each operation, from any such state (with arguments in range), gives the same contents as the reference,
/// keeps len <= cap, touches only allocated memory (Kani's pointer checks), and with the drop frees every block.
fn one_op(op: u8) {
    one_op_at(op, None)
}

/// (at: the index argument, when fixed)
fn one_op_at(op: u8, at: Option<usize>) {
    // (insert: one element fewer, CBMC runs out of memory with 3 + the shifting loops)
    let maxk = if op == 2 { MAXLEN - 2 } else { MAXLEN - 1 };
    unsafe { LIVE = 0 };
    {
        let (mut v, mut m) = build(maxk);
        let x: u8 = kani::any();
        let i = match at {
            Some(i) => i,
            None => (kani::any::<u8>() as usize) % (MAXLEN + 1),
        };
        let n = m.n;
        match op {
            0 => {
                v.push(x);
                m.a[n] = x;
                m.n += 1;
            }
            1 => {
                let r = v.pop();
                if n == 0 {
                    assert!(r.is_none());
                } else {
                    m.n -= 1;
                    assert_eq!(r, Some(m.a[n - 1]));
                }
            }
            2 => {
                kani::assume(i <= n);
                v.insert(i, x);
                let mut j = n;
                while j > i {
                    m.a[j] = m.a[j - 1];
                    j -= 1;
                }
                m.a[i] = x;
                m.n += 1;
            }
            3 => {
                kani::assume(i < n);
                let r = v.remove(i);
                assert_eq!(r, m.a[i]);
                for j in i..n - 1 {
                    m.a[j] = m.a[j + 1];
                }
                m.n -= 1;
            }
            4 => {
                kani::assume(i < n);
                let r = v.swap_remove(i);
                assert_eq!(r, m.a[i]);
                m.a[i] = m.a[n - 1];
                m.n -= 1;
            }
            5 => {
                v.truncate(i);
                if i < n {
                    m.n = i;
                }
            }
            _ => {
                v.reverse();
                for j in 0..n / 2 {
                    m.a.swap(j, n - 1 - j);
                }
            }
        }
        same(&v, &m);
    }
    assert_eq!(unsafe { LIVE }, 0, "a block was not freed");
}

#[kani::proof]
#[kani::unwind(6)]
fn push_matches_model() {
    one_op(0)
}
#[kani::proof]
#[kani::unwind(6)]
fn pop_matches_model() {
    one_op(1)
}
// (insert: one harness per index; with a symbolic index CBMC runs out of memory)
#[kani::proof]
#[kani::unwind(6)]
fn insert_0_matches_model() {
    one_op_at(2, Some(0))
}
#[kani::proof]
#[kani::unwind(6)]
fn insert_1_matches_model() {
    one_op_at(2, Some(1))
}
#[kani::proof]
#[kani::unwind(6)]
fn insert_2_matches_model() {
    one_op_at(2, Some(2))
}
#[kani::proof]
#[kani::unwind(6)]
#[kani::solver(cadical)]
fn remove_matches_model() {
    one_op(3)
}
#[kani::proof]
#[kani::unwind(6)]
fn swap_remove_matches_model() {
    one_op(4)
}
#[kani::proof]
#[kani::unwind(6)]
fn truncate_matches_model() {
    one_op(5)
}
#[kani::proof]
#[kani::unwind(6)]
#[kani::solver(cadical)]
fn reverse_matches_model() {
    one_op(6)
}

static mut DROPS: [u8; 4] = [0; 4];

struct D(usize);
impl Drop for D {
    fn drop(&mut self) {
        unsafe { DROPS[self.0] += 1 };
    }
}

/// Every element is dropped exactly once: by truncate / clear, by the Vec's drop, or by the caller of
/// pop / remove / swap_remove (which get it out by value).
#[kani::proof]
#[kani::unwind(6)]
fn drops_each_element_once() {
    unsafe { DROPS = [0; 4] };
    {
        let mut v: Vec<D> = Vec::new();
        for k in 0..4 {
            v.push(D(k));
        }
        let op: u8 = kani::any();
        let i: usize = kani::any();
        kani::assume(i < 4);
        match op % 5 {
            0 => drop(v.remove(i)),
            1 => drop(v.swap_remove(i)),
            2 => v.truncate(i),
            3 => drop(v.pop()),
            _ => v.reverse(),
        }
    }
    for k in 0..4 {
        assert_eq!(unsafe { DROPS[k] }, 1);
    }
}

/// `with_capacity(n)` for any n either gives room for n elements or stops (`panic!("capacity overflow")` past
/// isize::MAX bytes, as std; abort when malloc fails): never a block smaller than `cap`. (Before 2026-11-20,
/// `c * size_of::<T>()` wrapped: `Vec::<u64>::with_capacity(1 << 61)` then `push` wrote past a tiny block.)
#[kani::proof]
#[kani::should_panic]
fn with_capacity_any_size() {
    let n: usize = kani::any();
    let mut v: Vec<u64> = Vec::with_capacity(n);
    v.push(7);
    // (Kani's pointer checks are not panics: should_panic passes only if every size that does not stop writes
    // inside the block. What a Vec holds is checked by the *_matches_model harnesses)
    kani::assume(false);
}

/// `reserve(extra)` for any extra on a non-empty Vec, then a push: inside the block, or a stop (len + extra never wraps).
#[kani::proof]
#[kani::should_panic]
fn reserve_any_size() {
    let mut v: Vec<u8> = Vec::new();
    v.push(1);
    let extra: usize = kani::any();
    v.reserve(extra);
    v.push(2);
    kani::assume(false);
}

/// When malloc fails, the program stops (std calls `handle_alloc_error`, which aborts). (Before 2026-11-20,
/// `reserve` kept the null pointer and `push` wrote through it.)
#[kani::proof]
#[kani::should_panic]
fn alloc_failure_stops() {
    unsafe { ALLOC_MAY_FAIL = true };
    let mut v: Vec<u32> = Vec::new();
    v.push(1);
    kani::assume(false);
}

/// What `reserve` promises when it does not stop (the should_panic harnesses above only show that it never writes
/// outside the block): `reserve(n)` gives `capacity() >= len + n`, and n pushes after it fit in that block
/// (no reallocation, every write inside it: Kani's pointer checks).
#[kani::proof]
#[kani::unwind(8)]
fn reserve_gives_room() {
    unsafe { LIVE = 0 };
    {
        let mut v: Vec<u8> = Vec::new();
        v.push(1);
        let n = (kani::any::<u8>() as usize) % 7;
        v.reserve(n);
        let cap = v.capacity();
        assert!(cap >= 1 + n);
        for k in 0..6 {
            if k < n {
                v.push(k as u8);
            }
        }
        assert!(v.capacity() == cap);
        assert!(v.len() == 1 + n);
    }
    assert!(unsafe { LIVE } == 0, "a block was not freed");
}
