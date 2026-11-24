// Patterns from rand_core 0.10 and chacha20 0.10: a const generic parameter in a module named like a constant of
// another module; `Self::Seed::default()` in a trait's default method; an array's `as_mut()`; a statement-position
// `cfg_if!` that declares a `let` used after it
#[macro_export]
macro_rules! cfg_if {
    (
        if #[cfg( $($i_meta:tt)+ )] { $( $i_tokens:tt )* }
        $(
            else if #[cfg( $($ei_meta:tt)+ )] { $( $ei_tokens:tt )* }
        )*
        $(
            else { $( $e_tokens:tt )* }
        )?
    ) => {
        $crate::cfg_if! {
            @__items () ;
            (( $($i_meta)+ ) ( $( $i_tokens )* )),
            $(
                (( $($ei_meta)+ ) ( $( $ei_tokens )* )),
            )*
            $(
                (() ( $( $e_tokens )* )),
            )?
        }
    };

    // Internal and recursive macro to emit all the items
    //
    // Collects all the previous cfgs in a list at the beginning, so they can be
    // negated. After the semicolon are all the remaining items.
    (@__items ( $( ($($_:tt)*) , )* ) ; ) => {};
    (
        @__items ( $( ($($no:tt)+) , )* ) ;
        (( $( $($yes:tt)+ )? ) ( $( $tokens:tt )* )),
        $( $rest:tt , )*
    ) => {
        // Emit all items within one block, applying an appropriate #[cfg]. The
        // #[cfg] will require all `$yes` matchers specified and must also negate
        // all previous matchers.
        #[cfg(all(
            $( $($yes)+ , )?
            not(any( $( $($no)+ ),* ))
        ))]
        // Subtle: You might think we could put `$( $tokens )*` here. But if
        // that contains multiple items then the `#[cfg(all(..))]` above would
        // only apply to the first one. By wrapping `$( $tokens )*` in this
        // macro call, we temporarily group the items into a single thing (the
        // macro call) that will be included/excluded by the `#[cfg(all(..))]`
        // as appropriate. If the `#[cfg(all(..))]` succeeds, the macro call
        // will be included, and then evaluated, producing `$( $tokens )*`. See
        // also the "issue #90" test below.
        $crate::cfg_if! { @__temp_group $( $tokens )* }

        // Recurse to emit all other items in `$rest`, and when we do so add all
        // our `$yes` matchers to the list of `$no` matchers as future emissions
        // will have to negate everything we just matched as well.
        $crate::cfg_if! {
            @__items ( $( ($($no)+) , )* $( ($($yes)+) , )? ) ;
            $( $rest , )*
        }
    };

    // See the "Subtle" comment above.
    (@__temp_group $( $tokens:tt )* ) => {
        $( $tokens )*
    };
}

mod utils {
    pub fn read_words<W: Copy + Default, const N: usize>(src: &[u8]) -> [W; N] {
        assert_eq!(src.len(), core::mem::size_of::<[W; N]>());
        [W::default(); N]
    }
}

mod block {
    pub const N: usize = 2;
    pub fn first<const N: usize>(a: [u8; N]) -> u8 {
        a[0]
    }
}

pub trait Seedable: Sized {
    type Seed: Clone + Default + AsMut<[u8]>;
    fn from_seed(seed: Self::Seed) -> Self;
    fn seed_from_u64(state: u64) -> Self {
        let mut seed = Self::Seed::default();
        for (i, b) in seed.as_mut().iter_mut().enumerate() {
            *b = (state >> (i % 8 * 8)) as u8;
        }
        Self::from_seed(seed)
    }
}

struct Xs(u64);

impl Seedable for Xs {
    type Seed = [u8; 8];
    fn from_seed(seed: [u8; 8]) -> Self {
        Xs(u64::from_le_bytes(seed))
    }
}

struct S {
    state: u8,
    tokens: (),
}

impl S {
    fn new(state: u8) -> Self {
        cfg_if! {
            if #[cfg(target_arch = "x86_64")] {
                cfg_if! {
                    if #[cfg(chacha20_avx512)] {
                        let tokens = ((), ());
                    } else {
                        let tokens = ();
                    }
                }
            } else {
                let tokens = ();
            }
        }
        Self { state, tokens }
    }
}

fn main() {
    let w: [u32; 2] = utils::read_words(&[0u8; 8]);
    println!("{:?} {} {}", w, block::first([3u8; 4]), block::N);
    println!("{}", Xs::seed_from_u64(0x0102).0);
    let mut a = [1u8; 4];
    for b in a.as_mut().iter_mut() {
        *b += 1;
    }
    a.as_mut()[0] = 9;
    println!("{:?}", a);
    let s = S::new(3);
    println!("{} {:?}", s.state, s.tokens);
}
