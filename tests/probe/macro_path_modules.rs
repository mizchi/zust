// The modules of a macro's path are a path's (the review of #251): the name before `!` is a macro, the modules
// before it are renamed with their module. Two fn bodies' own `mod m` with an `m::say!(..)` each, and a module that
// brings std's `println` as `say!` beside its own `fn say` (which n's `fn say` makes the flattening rename), by
// `m::`, `crate::m::` and `self::`
mod m {
    pub use std::println as say;
    pub fn say() -> u8 {
        1
    }
    pub fn twice() {
        self::say!("twice {}", say());
        crate::m::say!("twice {}", self::say() + 1);
    }
}

mod n {
    pub fn say() -> u8 {
        2
    }
}

fn a() -> u8 {
    mod m {
        pub use std::println as say;
        pub fn f() -> u8 {
            1
        }
    }
    m::say!("hello a");
    m::f()
}

fn b() -> u8 {
    mod m {
        pub use std::println as say;
        pub fn f() -> u8 {
            2
        }
    }
    m::say!("hello b");
    m::f()
}

fn main() {
    m::say!("{} {}", m::say(), n::say());
    crate::m::say!["{}", a() + b()];
    m::twice();
}
