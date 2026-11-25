// crates/base/src/par.rs の代わり (セルフホスト): zustc のスレッドは green thread (OS のスレッドは 1 つ) で
// `thread::scope` もないので、並列の map は呼んだスレッドでの順のループにする。結果は par.rs の jobs 1 と同じ。
// (issue #165。rustc 版と zustc 版の両方がこれを使うので、2 つのコンパイラの出力は比べられる)

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Fork,
    Threads,
    Off,
}

/// ZUSTC_PAR の値 (threads もここでは順のループで動く)
pub fn mode() -> Mode {
    match std::env::var("ZUSTC_PAR") {
        Ok(v) if v == "threads" || v == "thread" => Mode::Threads,
        Ok(v) if v == "off" || v == "0" => Mode::Off,
        _ => Mode::Fork,
    }
}

pub fn spawned() -> usize {
    0
}

pub fn map_with<S, T>(_jobs: usize, n: usize, _chunk: usize, init: &dyn Fn() -> S, f: &dyn Fn(&mut S, usize) -> T) -> Vec<Option<T>> {
    let mut st = init();
    let mut out: Vec<Option<T>> = Vec::new();
    let mut i = 0;
    while i < n {
        out.push(Some(f(&mut st, i)));
        i += 1;
    }
    out
}

pub fn map_until<S, T>(_jobs: usize, n: usize, _chunk: usize, init: &dyn Fn() -> S, f: &dyn Fn(&mut S, usize) -> Option<T>) -> Vec<Option<T>> {
    let mut st = init();
    let mut out: Vec<Option<T>> = Vec::new();
    let mut i = 0;
    let mut failed = false;
    while i < n {
        if failed {
            out.push(None);
        } else {
            let r = f(&mut st, i);
            failed = r.is_none();
            out.push(r);
        }
        i += 1;
    }
    out
}

pub fn map<T>(jobs: usize, n: usize, chunk: usize, init: &dyn Fn(), f: &dyn Fn(usize) -> T) -> Vec<Option<T>> {
    map_with(jobs, n, chunk, &|| init(), &|_: &mut (), i| f(i))
}
