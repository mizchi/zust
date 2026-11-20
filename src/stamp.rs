//! A no-op build that reads no source: `<output>.stamp` keeps the stat stamps (device, inode, size, mtime, ctime,
//! mode) of every file the last successful build read, of the places `mod x;` looked (present or not), of the zustc
//! binary and of the output. When the command line, the environment that steers zustc and every one of those stamps
//! are as kept, the output is fresh and nothing else is read (the way blaze's file watcher skips a file whose stamp
//! did not move, crates/blaze-vfs/src/watch.rs).
//!
//! - Racy stamps (git's rule, by the file system's clock as blaze-vfs's `window_ns`): a stamp taken within the
//!   window of its file's change could be left by a second write in the same clock tick. A build that read such a
//!   file keeps no stamp file, so the next build does the whole work and keeps one then.
//! - A crate whose result depends on more than its files (`include!`, `env!`) keeps none.
//! - The stamp file goes before a build that does the work and comes back only after it succeeded, so a failed or
//!   interrupted build never leaves a stale one.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const HEAD: &str = "zustc-stamp 1";
/// whole-second stamps (a coarse clock): 2 s, else twice the slowest kernel tick
const COARSE_NS: i128 = 2_000_000_000;
const FINE_NS: i128 = 20_000_000;

/// (dev, ino, size, mtime ns, ctime ns, mode), or None when the path does not exist
fn stamp(p: &Path) -> Option<[i128; 6]> {
    let m = std::fs::metadata(p).ok()?;
    Some([
        m.dev() as i128,
        m.ino() as i128,
        m.size() as i128,
        m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128,
        m.ctime() as i128 * 1_000_000_000 + m.ctime_nsec() as i128,
        m.mode() as i128,
    ])
}

fn show(s: Option<[i128; 6]>) -> String {
    match s {
        Some(a) => a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" "),
        None => "-".to_string(),
    }
}

/// what else decides the output: the command line, the working directory, the environment zustc reads, the binary
pub fn key(args: &[String]) -> String {
    let mut k = String::new();
    // (--time only reports)
    for a in args.iter().filter(|a| *a != "--time") {
        k.push_str(a);
        k.push('\u{1f}');
    }
    k.push_str(&std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default());
    let mut env: Vec<(String, String)> = std::env::vars()
        .filter(|(n, _)| n.starts_with("ZUSTC_") || n.starts_with("ACG_") || n.starts_with("MINCLIF_") || n == "PATH")
        .collect();
    env.sort();
    for (n, v) in env {
        k.push('\u{1f}');
        k.push_str(&n);
        k.push('=');
        k.push_str(&v);
    }
    let exe = std::env::current_exe().ok();
    k.push('\u{1f}');
    k.push_str(&show(exe.as_deref().and_then(stamp)));
    k.replace('\n', "\u{1e}")
}

/// is the output as the stamp file says (nothing it names moved)?
pub fn fresh(file: &Path, key: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else { return false };
    let mut lines = text.lines();
    if lines.next() != Some(HEAD) || lines.next() != Some(key) {
        return false;
    }
    let mut any = false;
    for l in lines {
        let Some((st, p)) = l.split_once('\t') else { return false };
        if show(stamp(Path::new(p))) != st {
            return false;
        }
        any = true;
    }
    any
}

/// keep the stamps after a successful build (none when one of them is racy or the paths can't be written)
pub fn save(file: &Path, key: &str, inputs: &[PathBuf], probes: &[PathBuf], output: &Path) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    let mut out = format!("{HEAD}\n{key}\n");
    // (the output is this build's own: only the inputs can be racy)
    let n_in = inputs.len() + probes.len();
    for (i, p) in inputs.iter().chain(probes).map(|p| p.as_path()).chain([output]).enumerate() {
        let s = p.to_str().unwrap_or("\n");
        if s.contains('\n') || s.contains('\t') {
            return;
        }
        let st = stamp(p);
        if let Some(a) = st.filter(|_| i < n_in) {
            let window = if a[3] % 1_000_000_000 == 0 && a[4] % 1_000_000_000 == 0 { COARSE_NS } else { FINE_NS };
            if now - a[3].max(a[4]) < window {
                return;
            }
        }
        out.push_str(&show(st));
        out.push('\t');
        out.push_str(s);
        out.push('\n');
    }
    // (a tmp name of this process's own: another zustc saving the same stamp must not write into the file we rename)
    let tmp = file.with_extension(format!("stamp.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, out).is_err() || std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}
