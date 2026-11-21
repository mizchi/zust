//! CVE-2022-21658 (std, fixed in 1.58.1): `fs::remove_dir_all` checked whether an entry was a symlink and then
//! removed it recursively, so a symlink swapped in between made it delete outside the tree (a TOCTOU race). zustc's
//! prelude has no remove_dir_all, so the build is rejected. When it gets one, this case fails: make it a
//! test of the race (a symlink swapped in during the walk must not be followed).
//@ cve: CVE-2022-21658
//@ expect: unsupported
fn main() {
    let r = std::fs::remove_dir_all("zust_cve_2022_21658_probe");
    println!("{}", r.is_err());
}
