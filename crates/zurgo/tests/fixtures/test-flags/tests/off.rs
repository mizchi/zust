// (`test = false` on a `[[test]]`: not run)
#[test]
fn never() {
    panic!("this test target is off");
}
