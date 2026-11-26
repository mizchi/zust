// `AsMut<[T]>` of arrays, slices (through `&mut`) and Vec, as a bound: bindgen's bitfield unit (linux-raw-sys),
// `Storage: AsRef<[u8]> + AsMut<[u8]>`
struct BitfieldUnit<Storage> {
    storage: Storage,
}

impl<Storage> BitfieldUnit<Storage>
where
    Storage: AsRef<[u8]> + AsMut<[u8]>,
{
    fn get_bit(&self, index: usize) -> bool {
        let byte = self.storage.as_ref()[index / 8];
        byte & (1 << (index % 8)) != 0
    }
    fn set_bit(&mut self, index: usize, val: bool) {
        let byte = &mut self.storage.as_mut()[index / 8];
        let mask = 1 << (index % 8);
        if val {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }
    fn len(&self) -> usize {
        self.storage.as_ref().len()
    }
}

fn zero<T: AsMut<[i32]>>(mut t: T) -> T {
    for x in t.as_mut() {
        *x = 0;
    }
    t
}

fn bump<T: AsMut<[i32]> + ?Sized>(t: &mut T) {
    for x in t.as_mut().iter_mut() {
        *x += 1;
    }
}

fn main() {
    let mut u = BitfieldUnit { storage: [0u8; 2] };
    u.set_bit(1, true);
    u.set_bit(9, true);
    u.set_bit(1, false);
    u.set_bit(3, true);
    println!("{} {} {} {} {:?}", u.get_bit(1), u.get_bit(3), u.get_bit(9), u.len(), u.storage);
    let mut v = BitfieldUnit { storage: vec![0u8; 3] };
    v.set_bit(17, true);
    println!("{:?} {}", v.storage, v.get_bit(17));
    println!("{:?}", zero([5, 6, 7]));
    println!("{:?}", zero(vec![1, 2]));
    let mut a = [1, 2, 3];
    bump(&mut a);
    bump(&mut a[..2]);
    println!("{:?}", a);
    let mut w = vec![10, 20];
    bump(&mut w);
    let s: &mut [i32] = w.as_mut();
    s[1] = 0;
    println!("{:?}", w);
    let mut arr = [1u8, 2];
    let s: &mut [u8] = arr.as_mut();
    s[0] = 7;
    println!("{:?}", arr);
    println!("{:?}", zero(&mut [4, 5]));
}
