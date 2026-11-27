// Outer attributes on the expressions of a list: a call's and a method call's arguments, an array's elements
// (itertools' `self.try_fold(init, #[inline(always)] |acc, v| ..)`)
use std::ops::ControlFlow::{self, Break, Continue};

fn apply<F: Fn(i32) -> i32>(x: i32, f: F) -> i32 {
    f(x)
}

fn fold_while(init: i32, f: impl Fn(i32, i32) -> ControlFlow<i32, i32>) -> ControlFlow<i32, i32> {
    let mut acc = init;
    for v in 1..10 {
        match f(acc, v) {
            Continue(a) => acc = a,
            Break(a) => return Break(a),
        }
    }
    Continue(acc)
}

struct W;

impl W {
    fn m(&self, x: i32, f: impl Fn(i32) -> i32) -> i32 {
        f(x)
    }
}

fn main() {
    let a = apply(
        3,
        #[inline(always)]
        |v| v * 2,
    );
    let b = W.m(1, #[inline] |v| v + 10);
    let c = [#[allow(unused)] 1, #[allow(unused)] 2];
    let d = [#[allow(unused_parens)] (4)];
    let t = (#[allow(unused)] 5, 6);
    let r = fold_while(
        0,
        #[inline(always)]
        |acc, v| if acc > 10 { Break(acc) } else { Continue(acc + v) },
    );
    println!("{} {} {:?} {:?} {:?} {:?}", a, b, c, d, t, r);
}
