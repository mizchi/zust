// std::sync's LockResult / TryLockResult, PoisonError (new, into_inner, get_ref, Display, Debug), TryLockError, and the
// locks' try_lock / try_read / try_write / is_poisoned (#250: crossbeam-utils' ShardedLock)
use std::sync::{LockResult, Mutex, PoisonError, RwLock, TryLockError, TryLockResult};
struct Shard<T> { lock: RwLock<T> }
fn into_inner<T>(s: Shard<T>) -> LockResult<T> {
    let poisoned = s.lock.is_poisoned();
    let inner = s.lock.into_inner().unwrap();
    if poisoned { Err(PoisonError::new(inner)) } else { Ok(inner) }
}
fn try_read_len(s: &Shard<Vec<u8>>) -> TryLockResult<usize> {
    match s.lock.try_read() {
        Ok(g) => Ok(g.len()),
        Err(TryLockError::Poisoned(e)) => Err(TryLockError::Poisoned(PoisonError::new(e.into_inner().len()))),
        Err(TryLockError::WouldBlock) => Err(TryLockError::WouldBlock),
    }
}
fn main() {
    let m = Mutex::new(5);
    {
        let g = m.lock().unwrap();
        println!("{} {}", *g, m.try_lock().is_err());
        if let Err(TryLockError::WouldBlock) = m.try_lock() { println!("would block"); }
    }
    *m.try_lock().unwrap() += 1;
    println!("{} {}", *m.lock().unwrap(), m.is_poisoned());
    let s = Shard { lock: RwLock::new(vec![1u8, 2, 3]) };
    println!("{:?}", try_read_len(&s).ok());
    {
        let _w = s.lock.write().unwrap();
        println!("{}", try_read_len(&s).is_err());
    }
    let e: PoisonError<u8> = PoisonError::new(9);
    println!("{} {:?} {}", e, e, e.get_ref());
    let t: TryLockError<u8> = TryLockError::WouldBlock;
    println!("{}", t);
    println!("{:?}", into_inner(s));
    more();
}

// (std's Err types: `Err(e) => e.into_inner()` of a read, `unwrap_or_else(|e| e.into_inner())` of a lock and a
// Condvar wait, and From<PoisonError<T>> for TryLockError<T>)
fn read_or_recover(l: &RwLock<u8>) -> u8 {
    match l.read() {
        Ok(g) => *g,
        Err(e) => *e.into_inner(),
    }
}
#[allow(dead_code)]
fn cv_wait(m: &Mutex<bool>, c: &std::sync::Condvar) -> LockResult<()> {
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    while !*g {
        g = c.wait(g).unwrap_or_else(|e| e.into_inner());
    }
    Ok(())
}
pub fn more() {
    let l = RwLock::new(4u8);
    let t: TryLockError<u8> = PoisonError::new(3).into();
    println!("{} {:?}", read_or_recover(&l), t);
    let w = l.try_write();
    println!("{} {}", w.is_ok(), l.try_read().is_err());
    drop(w);
    println!("{} {}", l.try_write().is_ok(), l.is_poisoned());
}
