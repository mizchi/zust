// A `_` parameter passed on as another value (an enum variant, not the caller's): not a forward, so the impl's method runs (rustc: 1) (#144)
#[derive(Clone, Copy)]
enum Mode { Fast, Slow }
trait Go { fn go(&self, _: Mode) -> u32; }
struct G;
impl Go for G { fn go(&self, m: Mode) -> u32 { match m { Mode::Fast => 1, Mode::Slow => 2 } } }
impl<T: Go + ?Sized> Go for &T {
    fn go(&self, _: Mode) -> u32 { (**self).go(Mode::Fast) }
}
fn run<X: Go>(x: X) -> u32 { x.go(Mode::Slow) }
fn main() { println!("{}", run(&G)); }
