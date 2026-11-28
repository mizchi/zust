// std::any::Any without threads: `is` and type_id of `&dyn Any` (also named by its path only), `{:?}` of a
// `Box<dyn Any + Send>` (std's `Any { .. }`, alone, inside an Err, through a `T: Debug + ?Sized` bound, and `+ Send + Sync`), a `T:
// Any` bound (also `?Sized`: str and slices are Any, `<str as Any>::type_id`), and an Instant before the
// clock's origin (`now - 10 years`), which std's signed seconds can hold, `-=` and an elapsed f64 (#250)
use std::any::Any;
use std::time::{Duration, Instant};

fn show(x: &dyn std::any::Any) -> bool {
    x.is::<u8>()
}

fn same<T: Any>(t: &T) -> bool {
    (t as &dyn Any).is::<u8>()
}

fn dbg<T: std::fmt::Debug + ?Sized>(x: &T) -> String {
    format!("{:?}", x)
}

fn tid<T: Any + ?Sized>(x: &T) -> std::any::TypeId {
    x.type_id()
}

fn main() {
    let b: Box<dyn std::any::Any> = Box::new(5u8);
    println!("{} {} {}", show(&3u8), show(&3u16), (*b).type_id() == std::any::TypeId::of::<u8>());
    let e: Box<dyn Any + Send> = Box::new(3u8);
    println!("{:?} {}", e, e.is::<u8>());
    let r: Result<u8, Box<dyn Any + Send>> = Err(Box::new(1));
    println!("{:?}", r);
    println!("{} {}", same(&4u8), same(&"s"));
    let x: &dyn Any = &1u8;
    println!("{} {}", dbg(x), dbg(&e));
    let ss: &(dyn Any + Send + Sync) = &2u8;
    let bs: Box<dyn Any + Sync + Send> = Box::new(3u8);
    println!("{:?} {:?} {}", ss, bs, ss.is::<u8>());
    let mut t = Instant::now() + Duration::from_secs(5);
    t -= Duration::from_secs(5);
    println!("{}", format!("{}", t.elapsed().as_secs_f64()).len() > 0);
    let id = <str as Any>::type_id("abc");
    println!("{} {} {}", id == std::any::TypeId::of::<str>(), id == std::any::TypeId::of::<&str>(), tid(&[1u8, 2][..]) == std::any::TypeId::of::<[u8]>());

    let now = Instant::now();
    let ten = Duration::from_secs(10 * 365 * 24 * 3600);
    let past = now.checked_sub(ten);
    println!("{} {:?}", past.is_some(), past.map(|p| now.duration_since(p) == ten));
    println!("{}", past.map_or(false, |p| p < now));
}
