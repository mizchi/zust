// An associated type of a generic impl (`impl<R> Generator for Core<R> { type Output = [u32; 4]; }`) as the type of a
// field of a generic struct (`BlockRng<G> { results: G::Output }`) used in a non-generic struct's field (rand_core
// 0.10's BlockRng in chacha20's ChaCha8Rng)
pub trait Generator {
    type Output;
    fn generate(&mut self, out: &mut Self::Output);
    fn drop(&mut self, output: &mut Self::Output) {
        let _ = output;
    }
}

#[derive(Clone)]
pub struct BlockRng<G: Generator> {
    results: G::Output,
    pub core: G,
}

impl<G: Generator> Drop for BlockRng<G> {
    fn drop(&mut self) {
        self.core.drop(&mut self.results);
    }
}

pub trait Word: Copy + Default {
    fn from_usize(n: usize) -> Self;
}
impl Word for u32 {
    fn from_usize(n: usize) -> Self {
        n as u32
    }
}

impl<W: Word + Default, const N: usize, G: Generator<Output = [W; N]>> BlockRng<G> {
    pub fn new(core: G) -> BlockRng<G> {
        let mut results = [W::default(); N];
        results[0] = W::from_usize(N);
        BlockRng { core, results }
    }
    pub fn next(&mut self) -> W {
        self.core.generate(&mut self.results);
        self.results[1]
    }
}

#[derive(Clone)]
struct Core<R>(u32, core::marker::PhantomData<R>);

#[derive(Clone)]
struct R8;

impl<R> Generator for Core<R> {
    type Output = [u32; 4];
    fn generate(&mut self, out: &mut [u32; 4]) {
        self.0 += 1;
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.0 * 10 + i as u32;
        }
    }
}

struct Rng8 {
    core: BlockRng<Core<R8>>,
}

impl Rng8 {
    fn from_seed(x: u32) -> Self {
        let core = Core(x, core::marker::PhantomData);
        Self { core: BlockRng::new(core) }
    }
}

fn main() {
    let mut g = Rng8::from_seed(5);
    println!("{}", g.core.next());
    let mut r = BlockRng::new(Core::<R8>(0, core::marker::PhantomData));
    let a = r.results[0];
    println!("{} {}", a, r.next());
    println!("{}", r.next());
}
