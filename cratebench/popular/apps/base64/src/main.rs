use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE};

fn main() {
    for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
        println!("{:?} -> {}", s, STANDARD.encode(s));
    }
    let bin: Vec<u8> = (0..=255u8).step_by(17).collect();
    let u = URL_SAFE.encode(&bin);
    println!("url-safe {}", u);
    println!("round trip {}", URL_SAFE.decode(&u).unwrap() == bin);
    println!("no pad {}", STANDARD_NO_PAD.encode(b"hi"));
    println!("bad input rejected: {}", STANDARD.decode("@@@").is_err());
    println!("done");
}
