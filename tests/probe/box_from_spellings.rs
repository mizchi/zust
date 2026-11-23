// `Box::from` of a slice spelled with a turbofish or spaces, alone in the program: the prelude's slice-copy names
// (`to_vec`, `into_boxed_slice`) are selected for each
fn main() {
    let a: &[u8] = &[4, 5];
    let b: Box<[u8]> = Box:: from(a);
    let c: Box<[u8]> = Box :: from(&a[1..]);
    let d = Box::<[u8]>::from(&a[..1]);
    println!("{:?} {:?} {:?}", &b[..], &c[..], &d[..]);
}
