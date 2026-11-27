// std::fs::File, OpenOptions and Metadata (the crate survey's #250 `File`: cc, same-file, quick-xml, chrono, ...),
// with std::os::unix's fd traits and MetadataExt / OpenOptionsExt / PermissionsExt. The scratch file is named after
// the executable, so each build of this program has its own
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

struct Handle {
    file: Option<File>,
    dev: u64,
    ino: u64,
}

impl Handle {
    fn from_file(file: File) -> io::Result<Handle> {
        let md = file.metadata()?;
        Ok(Handle { file: Some(file), dev: md.dev(), ino: md.ino() })
    }
    fn fd(&self) -> RawFd {
        self.file.as_ref().unwrap().as_raw_fd()
    }
}

fn main() -> io::Result<()> {
    let path = format!("{}.std_file.tmp", std::env::args().next().unwrap());
    {
        let mut f = File::create(&path)?;
        f.write_all(b"hello\nworld\n")?;
        f.flush()?;
        f.sync_all()?;
    }
    let mut s = String::new();
    File::open(&path)?.read_to_string(&mut s)?;
    print!("{}", s);

    let mut f = OpenOptions::new().append(true).open(&path)?;
    f.write_all(b"more\n")?;
    drop(f);

    let mut f = OpenOptions::new().read(true).write(true).open(&path)?;
    let md = f.metadata()?;
    println!("len {} file {} dir {} readonly {}", md.len(), md.is_file(), md.is_dir(), md.permissions().readonly());
    println!("size {} nlink {} type bits {:o}", md.size(), md.nlink(), md.mode() & 0o170000);
    let mut buf = [0u8; 5];
    f.seek(SeekFrom::Start(6))?;
    f.read_exact(&mut buf)?;
    println!("{:?} at {}", std::str::from_utf8(&buf).unwrap(), f.stream_position()?);
    let end = f.seek(SeekFrom::End(-5))?;
    let mut rest = Vec::new();
    f.read_to_end(&mut rest)?;
    println!("end-5 = {} {:?}", end, String::from_utf8_lossy(&rest));
    f.set_len(5)?;
    println!("after set_len: {}", f.metadata()?.len());

    let a = Handle::from_file(File::open(&path)?)?;
    let b = Handle::from_file(File::open(&path)?)?;
    println!("same file: {} fds differ: {}", (a.dev, a.ino) == (b.dev, b.ino), a.fd() != b.fd());

    let mut c = OpenOptions::new();
    c.write(true).create_new(true).mode(0o600);
    match c.open(&path) {
        Ok(_) => println!("create_new opened an existing file"),
        Err(e) => println!("create_new: {} {:?}", e, e.raw_os_error()),
    }
    match OpenOptions::new().open(&path) {
        Ok(_) => println!("no access mode opened"),
        Err(e) => println!("no access mode: {} {:?}", e, e.raw_os_error()),
    }
    match OpenOptions::new().read(true).create(true).open(&path) {
        Ok(_) => println!("create without write opened"),
        Err(e) => println!("create without write: {} {:?}", e, e.raw_os_error()),
    }
    match OpenOptions::new().append(true).truncate(true).open(&path) {
        Ok(_) => println!("append and truncate opened"),
        Err(e) => println!("{:?}", e),
    }
    match File::open(format!("{}.missing", path)) {
        Ok(_) => println!("missing opened"),
        Err(e) => println!("missing: {} {:?} {:?}", e, e.raw_os_error(), e),
    }
    let p = std::fs::metadata(&path)?.permissions();
    println!("mode {:o}", p.mode() & 0o777 & !0o022);

    // (stdout's fd as a File, given back unclosed; what println! wrote is flushed first, as zustc's stdout is not
    // line-buffered into a pipe or a file)
    io::stdout().flush()?;
    let mut out = unsafe { File::from_raw_fd(1) };
    out.write_all(b"through fd 1\n")?;
    let fd = out.into_raw_fd();
    println!("fd {}", fd);
    std::fs::remove_file(&path)?;
    println!("removed: {}", File::open(&path).is_err());
    Ok(())
}
