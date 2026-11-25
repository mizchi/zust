//! Linking.
//!
//! Going through the `cc` driver starts 3 levels of processes every time: gcc driver → collect2 → ld.
//! Only the first time, `cc -###` is used to get the ld command line the driver builds, which is
//! saved to `<out>.link`, and after that the linker (wild / ld.lld / ld) is exec'd directly.
//! (In a 1000-fn example, cc+lld 21ms → wild directly 5ms)

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn which(name: &str) -> Option<PathBuf> {
    if Path::new(name).is_absolute() {
        return Some(name.into());
    }
    let p = std::env::var_os("PATH")?;
    std::env::split_paths(&p)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// The linker executable to use. The default is wild > mold > ld.lld > ld.
pub fn pick(requested: Option<&str>) -> Result<PathBuf, String> {
    let cands: Vec<&str> = match requested {
        Some("lld") => vec!["ld.lld"],
        Some("bfd") => vec!["ld.bfd", "ld"],
        Some(l) => vec![l],
        None => vec!["wild", "mold", "ld.lld", "ld"],
    };
    cands
        .iter()
        .find_map(|c| which(c))
        .ok_or_else(|| format!("linker not found: {cands:?}"))
}

// (libm is needed only when floating-point fns are used: with --as-needed it is not linked if unused)
const DRIVER_FLAGS: &[&str] = &["-no-pie", "-s", "-Wl,--gc-sections", "-Wl,--build-id=none", "-Wl,--as-needed", "-lm"];

/// Extracts collect2's arguments from the output of `cc -###` and returns them with the plugin-related ones removed.
fn driver_args(objs: &[&Path], out: &Path) -> Result<Vec<String>, String> {
    let o = Command::new("cc")
        .arg("-###")
        .args(objs)
        .arg("-o")
        .arg(out)
        .args(DRIVER_FLAGS)
        .output()
        .map_err(|e| format!("failed to run cc: {e}"))?;
    let text = String::from_utf8_lossy(&o.stderr);
    let line = text
        .lines()
        .find(|l| l.contains("collect2") || l.trim_start().starts_with("\"/") && l.contains(" -o "))
        .ok_or("could not find the link line in `cc -###` output")?;
    let mut args = vec![];
    let mut skip = false;
    for (i, a) in shell_split(line).into_iter().enumerate() {
        if i == 0 || std::mem::take(&mut skip) {
            continue;
        }
        if a == "-plugin" {
            skip = true;
            continue;
        }
        if a.starts_with("-plugin-opt") || a.starts_with("-fuse-ld") {
            continue;
        }
        args.push(a);
    }
    Ok(args)
}

/// The output of `cc -###` mixes arguments enclosed in `"..."` and bare arguments.
fn shell_split(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_q = false;
    let mut any = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_q = !in_q;
                any = true;
            }
            '\\' if in_q => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !in_q => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn link(requested: Option<&str>, obj: &Path, out: &Path) -> Result<String, String> {
    link_many(requested, &[obj], out)
}

/// Makes several .o files into one executable
pub fn link_many(requested: Option<&str>, objs: &[&Path], out: &Path) -> Result<String, String> {
    let linker = pick(requested)?;
    let mut lpath = out.as_os_str().to_owned();
    lpath.push(".link");
    let lpath = PathBuf::from(lpath);
    // The saved link line depends only on the (obj, out) paths
    let mut header = String::new();
    for o in objs {
        header.push_str(&format!("{}\n", o.display()));
    }
    header.push_str(&format!("{}\n", out.display()));
    let args = match std::fs::read_to_string(&lpath) {
        Ok(s) if s.starts_with(&header) => s[header.len()..].lines().map(String::from).collect(),
        _ => {
            let a = driver_args(objs, out)?;
            let _ = std::fs::write(&lpath, format!("{header}{}", a.join("\n")));
            a
        }
    };
    // (For profiling: if ZUSTC_NO_STRIP is set, keep the symbols)
    let args: Vec<String> = if std::env::var_os("ZUSTC_NO_STRIP").is_some() {
        args.into_iter().filter(|a| a != "-s").collect()
    } else {
        args
    };
    let st = Command::new(&linker)
        .args(&args)
        .status()
        .map_err(|e| format!("failed to run {}: {e}", linker.display()))?;
    if !st.success() {
        let _ = std::fs::remove_file(&lpath);
        return Err(format!("{} failed: {st}", linker.display()));
    }
    Ok(linker.file_name().unwrap().to_string_lossy().into_owned())
}
