fn main() {
    let mut a = 1;
    let x: Option<&mut i32> = Some(&mut a);
    let _ = [x; 2];
}
