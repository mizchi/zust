// zustc renames std names its prelude has under others (`fmt::Write`, mpsc's `Sender`, atomic's `Ordering`) in the
// code's paths, a macro's spaced `core :: fmt :: Write` too, and never in a literal's text: a string's, a raw
// string's or a byte string's is printed as written, whether a `use` imports the name from the module (mpsc's here)
// or not (fmt's and atomic's)
use std::sync::mpsc::{Sender, channel};

struct Out(String);

// (spaced, as a macro's expansion writes the path)
impl core :: fmt :: Write for Out {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.push_str(s);
        Ok(())
    }
}

fn send(tx: &Sender<&'static str>, s: &'static str) {
    tx.send(s).unwrap();
}

fn main() {
    // fmt::Write and fmt :: Write in a comment
    /* std::sync::atomic::Ordering */
    println!("fmt :: Write");
    println!("io :: Write");
    println!("fmt::Write");
    println!("{} {}", "std::fmt::Write", "core :: fmt :: Write");
    println!(r"fmt::Write {}", 1);
    println!(r#"fmt :: Write "{}""#, 2);
    println!("{:?}", b"fmt::Write");
    let mut o = Out(String::new());
    std::fmt::Write::write_str(&mut o, "fmt::Write").unwrap();
    std::fmt::Write::write_fmt(&mut o, format_args!(" [{}]", "fmt :: Write")).unwrap();
    println!("{}", o.0);
    let (tx, rx) = channel();
    send(&tx, "mpsc::Sender, Sender");
    println!("{} {}", rx.recv().unwrap(), "mpsc :: Receiver");
    let a = std::sync::atomic::AtomicUsize::new(3);
    println!("{} {}", a.load(std::sync::atomic::Ordering::SeqCst), "atomic::Ordering");
    println!("{:?} {}", ['W', 'r'], 'W');
}
