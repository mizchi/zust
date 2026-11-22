// `cfg_select!`: the first arm whose predicate holds, as items, an expression or statements, nested too
cfg_select! {
    windows => {
        fn os() -> &'static str { "windows" }
    }
    unix => {
        fn os() -> &'static str { "unix" }
    }
    _ => {
        fn os() -> &'static str { "other" }
    }
}

fn width() -> u32 {
    cfg_select! {
        target_pointer_width = "32" => { 32 }
        target_pointer_width = "64" => { 64 }
        _ => { 0 }
    }
}

fn main() {
    let a = cfg_select! {
        false => { 1 }
        any(true, unix) => {
            cfg_select! {
                not(unix) => { 2 }
                _ => { 3 }
            }
        }
    };
    cfg_select! {
        test => { println!("test"); }
        _ => { println!("{} {} {}", os(), width(), a); }
    }
}
