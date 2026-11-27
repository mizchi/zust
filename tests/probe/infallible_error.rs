// `Infallible: Error` (and its Display), as rand_core's `impl<R: TryRngCore<Error = Infallible>>` needs: an error
// type bound by `Error` that is Infallible
use std::convert::Infallible;
use std::error::Error;
use std::fmt;

fn describe<E: Error>(e: &E) -> String {
    format!("{e} / {e:?}")
}

trait TryRng {
    type Error: Error;
    fn try_next(&mut self) -> Result<u32, Self::Error>;
}

struct Counter(u32);

impl TryRng for Counter {
    type Error = Infallible;
    fn try_next(&mut self) -> Result<u32, Infallible> {
        self.0 += 1;
        Ok(self.0)
    }
}

struct Wrap<E>(E);

impl<E: fmt::Debug> fmt::Debug for Wrap<E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Wrap({:?})", self.0)
    }
}
impl<E: fmt::Display> fmt::Display for Wrap<E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "wrapped {}", self.0)
    }
}
impl<E: Error> Error for Wrap<E> {}

fn next_or_report<R: TryRng>(r: &mut R) -> String {
    match r.try_next() {
        Ok(v) => format!("ok {v}"),
        Err(e) => describe(&e),
    }
}

fn never() -> Result<u8, Infallible> {
    Ok(3)
}

fn main() {
    let mut c = Counter(0);
    println!("{}", next_or_report(&mut c));
    println!("{}", next_or_report(&mut c));
    let r: Result<u8, Wrap<Infallible>> = never().map_err(Wrap);
    match r {
        Ok(v) => println!("ok {v}"),
        Err(e) => println!("{}", describe(&e)),
    }
    let b: Result<u8, Box<dyn Error>> = never().map_err(|e| e.into());
    println!("{:?}", b.is_ok());
    let n: Result<u32, Infallible> = "7".parse::<u32>().map_err(|_| unreachable!());
    println!("{:?}", n);
}
