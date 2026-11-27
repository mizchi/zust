// `char::REPLACEMENT_CHARACTER` (U+FFFD), and through char's deprecated module, `core::char::REPLACEMENT_CHARACTER`
// (#265: each was "cannot find value (unsupported path)"): in a fn (utf8_iter's handler, base64's decode error), in
// a const block a const fn reads (utf16_iter's `REPLACEMENT_CHARACTER_AS_BMP`), and in an integer const
#[derive(Clone, Copy)]
struct Bmp(u16);

impl Bmp {
    const fn try_new(x: u16) -> Result<Self, ()> {
        if x >= 0xD800 && x <= 0xDFFF { Err(()) } else { Ok(Bmp(x)) }
    }
}

const AS_BMP: Bmp = const {
    if let Ok(bmp) = Bmp::try_new(char::REPLACEMENT_CHARACTER as u16) {
        bmp
    } else {
        panic!()
    }
};

const CODE: u32 = char::REPLACEMENT_CHARACTER as u32 + 1;

trait Handler {
    fn error(&self) -> char;
}

struct Replace;

impl Handler for Replace {
    fn error(&self) -> char {
        char::REPLACEMENT_CHARACTER
    }
}

fn first_char(b: u8) -> char {
    core::str::from_utf8(&[b]).ok().and_then(|s| s.chars().next()).unwrap_or(core::char::REPLACEMENT_CHARACTER)
}

fn main() {
    println!("{} {} {}", first_char(b'A'), first_char(0xC3) as u32, std::char::REPLACEMENT_CHARACTER == Replace.error());
    println!("{:x} {:x} {}", AS_BMP.0, CODE, char::REPLACEMENT_CHARACTER.len_utf8());
    let s: String = [b'a', 0xFF, b'b'].iter().map(|&b| if b < 0x80 { b as char } else { Replace.error() }).collect();
    println!("{s} {}", s.chars().filter(|&c| c == char::REPLACEMENT_CHARACTER).count());
}
