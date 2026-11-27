// `{:p}` flags as std writes them: `+` before `0x` and within the width, `{:#p}` / `{:#?}` zero-extended, `{:010?}` of a
// raw pointer; a `dyn` pointer's `Pointer { addr, metadata: DynMetadata(..) }`, plain, padded and on lines
fn main() {
    let p = 0usize as *const u8;
    println!("[{:+p}] [{:+08p}] [{:+#p}] [{:#?}] [{:010?}]", p, p, p, p, p);
    let s: &[u8] = &[1];
    println!("{}", format!("{:+p}", s) == format!("Pointer {{ addr: {:+p}, metadata: +1 }}", s.as_ptr()));
    let x = 1;
    let d: &dyn std::fmt::Debug = &x;
    let a = d as *const dyn std::fmt::Debug as *const u8;
    let plain = format!("{:p}", d);
    let head = format!("Pointer {{ addr: {:p}, metadata: DynMetadata(0x", a);
    println!("{} {}", plain.starts_with(&head), plain.ends_with(") }"));
    let vt = &plain[head.len() - 2..plain.len() - 3];
    println!("{}", format!("[{:20p}]", d) == format!("[Pointer {{ addr: {:20p}, metadata: DynMetadata({:>20}) }}]", a, vt));
    let alt = format!("{:#p}", d);
    println!("{}", alt.starts_with("Pointer {\n    addr: 0x") && alt.contains(",\n    metadata: DynMetadata(\n        0x") && alt.ends_with(",\n    ),\n}"));
    let b: Box<dyn std::fmt::Debug> = Box::new(2);
    println!("{}", format!("{:p}", b).ends_with(&plain[plain.find("metadata").unwrap()..]));
}
