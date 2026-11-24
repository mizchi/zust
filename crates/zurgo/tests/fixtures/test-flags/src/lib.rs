pub fn one() -> i32 {
    1
}

// (`test = false` on the lib: never run)
#[cfg(test)]
mod tests {
    #[test]
    fn never() {
        panic!("the lib's tests are off");
    }
}
