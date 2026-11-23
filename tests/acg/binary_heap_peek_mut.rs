// `BinaryHeap::peek_mut`: a guard of the largest element, which the heap sifts down again when the guard is dropped
// after a change through it (and not after a read), as std's `PeekMut`
use std::collections::BinaryHeap;
fn main() {
    let mut heap: BinaryHeap<i32> = BinaryHeap::new();
    for x in [3, 9, 4, 7] {
        heap.push(x);
    }
    {
        let top = heap.peek_mut().unwrap();
        println!("{}", *top);
    }
    if let Some(mut top) = heap.peek_mut() {
        *top = 1;
    }
    println!("{:?}", heap.peek());
    let mut t = heap.peek_mut().unwrap();
    let r: &mut i32 = &mut *t;
    *r -= 10;
    drop(t);
    println!("{:?}", heap.into_sorted_vec());
    let mut empty: BinaryHeap<u8> = BinaryHeap::new();
    println!("{}", empty.peek_mut().is_none());
}
