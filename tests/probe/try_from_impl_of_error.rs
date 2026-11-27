// `?` converts the error with the `From` impl of the error's own type, also when the function's error type has
// several (base64ct's `str::from_utf8(b)?` beside its `From<InvalidLengthError>`), not with the group's first
#[derive(Debug)]
enum Error {
    Utf8(std::str::Utf8Error),
    Len(LenError),
    Code(u8),
}

#[derive(Debug)]
struct LenError;

impl From<LenError> for Error {
    fn from(e: LenError) -> Error {
        Error::Len(e)
    }
}

impl From<std::str::Utf8Error> for Error {
    fn from(e: std::str::Utf8Error) -> Error {
        Error::Utf8(e)
    }
}

impl From<u8> for Error {
    fn from(c: u8) -> Error {
        Error::Code(c)
    }
}

fn check_len(b: &[u8]) -> Result<usize, LenError> {
    if b.len() > 3 { Err(LenError) } else { Ok(b.len()) }
}

fn check_code(b: &[u8]) -> Result<(), u8> {
    if b.first() == Some(&b'!') { Err(7) } else { Ok(()) }
}

fn parse(b: &[u8]) -> Result<(&str, usize), Error> {
    let n = check_len(b)?;
    check_code(b)?;
    Ok((std::str::from_utf8(b)?, n))
}

fn main() {
    println!("{:?}", parse(b"ab").map(|x| x.1));
    println!("{}", matches!(parse(b"abcd"), Err(Error::Len(_))));
    println!("{}", matches!(parse(&[0xff]), Err(Error::Utf8(_))));
    println!("{:?}", parse(b"!x").err());
}
