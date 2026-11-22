pub fn value() -> u32 {
    if cfg!(spicy) { 2 } else { 1 }
}

pub fn flavor() -> &'static str {
    env!("GEN_FLAVOR")
}

pub fn compiled_with() -> &'static str {
    env!("GEN_COMPILED_WITH")
}
