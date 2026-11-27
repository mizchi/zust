// Derived Debug prints a program's names as written, also ones that start as the compiler's renamed ones do
// (`__p_` / `__u_`): only the names the compiler itself renamed lose the prefix
#![allow(non_camel_case_types)]

#[derive(Debug)]
enum State {
    __p_Ready,
    __u_Done(u8),
    Waiting { __p_left: u32 },
}

#[derive(Debug)]
struct __p_Raw(u8);

fn main() {
    println!("{:?} {:?} {:?}", State::__p_Ready, State::__u_Done(1), State::Waiting { __p_left: 2 });
    println!("{:?}", __p_Raw(3));
    println!("{:?}", std::thread::current().id());
}
