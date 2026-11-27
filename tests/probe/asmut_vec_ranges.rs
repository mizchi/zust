// `as_mut()` of a Vec as its AsMut<Vec<T>> where `&mut Vec<T>` is expected (beside an AsMut<[T]> bound), and of a
// range of a slice or an array (`s[..].as_mut()`), a `&mut` of the range
fn both<T: AsMut<[u8]> + AsMut<Vec<u8>>>(mut t: T) -> usize {
    let r: &mut Vec<u8> = t.as_mut();
    r.push(9);
    let s: &mut [u8] = t.as_mut();
    s[0] = 7;
    let v: &mut Vec<u8> = t.as_mut();
    v.len()
}

fn first(s: &mut [u8]) -> u8 {
    s[..].as_mut()[0] = 5;
    let a: &mut [u8] = s[1..].as_mut();
    a[0] = 6;
    s[0] + s[1]
}

fn main() {
    println!("{}", both(vec![1u8, 2]));
    let mut b = [1u8, 2, 3];
    println!("{} {:?}", first(&mut b), b);
    let mut arr = [0u8; 4];
    arr[..].as_mut()[1] = 3;
    let t: &mut [u8] = arr[2..].as_mut();
    t[0] = 4;
    println!("{:?}", arr);
    let mut v = vec![1u8, 2, 3];
    v[1..].as_mut()[0] = 9;
    println!("{} {:?}", v[..2].as_mut().len(), v);
    for x in arr[1..].as_mut().iter_mut() {
        *x += 1;
    }
    println!("{:?}", arr);
}
