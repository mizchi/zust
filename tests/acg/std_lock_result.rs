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
}
