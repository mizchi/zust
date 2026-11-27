// Edge cases of the prelude's File and Read, as std's (#291's review): a NUL inside a path is InvalidInput, a
// FileType is the type's bits only, try_clone's fd is close-on-exec, read_to_end reads again after Interrupted,
// read_to_string keeps what it read before an error; and `Self::m` in a trait's default method is the trait's
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;

unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32) -> i32;
}

struct Steps {
    steps: Vec<Result<&'static [u8], i32>>,
}

impl Read for Steps {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.steps.is_empty() {
            return Ok(0);
        }
        match self.steps.remove(0) {
            Ok(b) => {
                buf[..b.len()].copy_from_slice(b);
                Ok(b.len())
            }
            Err(code) => Err(io::Error::from_raw_os_error(code)),
        }
    }
}

trait Count {
    fn count_ones(self) -> u32;
    fn delegated(self) -> u32
    where
        Self: Sized,
    {
        Self::count_ones(self)
    }
}

impl Count for u32 {
    fn count_ones(self) -> u32 {
        100
    }
}

fn main() -> io::Result<()> {
    let base = format!("{}.std_file_edges", std::env::args().next().unwrap());
    match File::create(format!("{}\0x", base)) {
        Ok(_) => println!("NUL path opened"),
        Err(e) => println!("{} | {:?} | {:?}", e, e, e.raw_os_error()),
    }
    println!("{}", fs::metadata("a\0b").is_err());

    let (a, b) = (format!("{}.a", base), format!("{}.b", base));
    File::create(&a)?.write_all(b"1")?;
    File::create(&b)?.write_all(b"2")?;
    fs::set_permissions(&a, fs::Permissions::from_mode(0o600)).ok();
    let (ma, mb) = (fs::metadata(&a)?, fs::metadata(&b)?);
    println!("same file type: {}", ma.file_type() == mb.file_type());

    let f = File::open(&a)?;
    let g = f.try_clone()?;
    println!("clone close-on-exec: {}", unsafe { fcntl(g.as_raw_fd(), 1) } & 1 == 1);
    drop((f, g));
    fs::remove_file(&a)?;
    fs::remove_file(&b)?;

    let mut v = Vec::new();
    let n = Steps { steps: vec![Err(4), Ok(b"ab"), Err(4), Ok(b"cd")] }.read_to_end(&mut v)?;
    println!("{} {:?}", n, String::from_utf8_lossy(&v));
    let mut s = String::from(">");
    let r = Steps { steps: vec![Ok(b"abc"), Err(5)] }.read_to_string(&mut s);
    println!("{:?} {:?}", r.map_err(|e| e.raw_os_error()), s);
    let mut t = String::from(">");
    let r = Steps { steps: vec![Ok(b"a\xff"), Err(5)] }.read_to_string(&mut t);
    println!("{:?} {:?}", r.map_err(|e| e.raw_os_error()), t);

    println!("{} {}", Count::delegated(7u32), 7u32.count_ones());
    Ok(())
}
