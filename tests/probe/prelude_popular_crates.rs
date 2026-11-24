// std items that popular crates use and the prelude lacked: core::marker::PhantomPinned (libc 0.2),
// core::hint::spin_loop (foldhash 0.2, under hashbrown), ChunksExactMut::into_remainder (rand_core 0.10),
// TryFrom / TryInto implemented by a crate for its own type (base64 0.23).
use core::convert::TryFrom;
use core::hint::spin_loop;
use std::marker::{PhantomData, PhantomPinned};

#[derive(Debug, Clone, Copy)]
struct Opaque {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

#[derive(Debug, PartialEq)]
struct Alphabet(String);

#[derive(Debug, PartialEq)]
struct BadAlphabet(usize);

impl TryFrom<&str> for Alphabet {
    type Error = BadAlphabet;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        if s.len() == 4 { Ok(Alphabet(s.to_string())) } else { Err(BadAlphabet(s.len())) }
    }
}

fn main() {
    let o = Opaque { _data: (), _marker: PhantomData };
    println!("{:?}", o._data);
    spin_loop();
    core::hint::spin_loop();
    let mut buf = [1u8, 2, 3, 4, 5, 6, 7];
    let mut chunks = buf.chunks_exact_mut(3);
    for c in &mut chunks {
        c[0] = 0;
    }
    let rem = chunks.into_remainder();
    rem[0] = 9;
    println!("{:?}", buf);
    println!("{:?} {:?}", Alphabet::try_from("abcd"), Alphabet::try_from("abc"));
    let a: Result<Alphabet, BadAlphabet> = "wxyz".try_into();
    println!("{:?}", a);
    println!("{:?} {:?}", u8::try_from(200i32).is_ok(), u8::try_from(300i32).is_ok());
}
