//! Builds a multi-crate, multi-file workspace (tests/workspace) with `Session::build_workspace`, and checks whether
//! the output, exit code and panic messages (locations across crates and files) are the same as those of the one built with rustc (rlib + link).
//! Also, whether changing only a fn body in a crate that others depend on recompiles only that crate
#![cfg(feature = "acg")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let q = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &q);
        } else {
            std::fs::copy(&p, &q).unwrap();
        }
    }
}

fn run(exe: &Path, dir: &Path, n: usize) -> (String, String, Option<i32>) {
    let o = Command::new(exe).args(std::iter::repeat("x").take(n)).current_dir(dir).env_remove("RUST_BACKTRACE").output().unwrap();
    // (newer rustc prints the thread ID: `thread 'main' (1234) panicked`)
    let mut err = String::from_utf8_lossy(&o.stderr).into_owned();
    if let Some(i) = err.find("thread 'main' (") {
        if let Some(j) = err[i..].find(") ") {
            err.replace_range(i + 13..i + j + 1, "");
        }
    }
    (String::from_utf8_lossy(&o.stdout).into_owned(), err, o.status.code())
}

#[test]
fn workspace_matches_rustc_and_rebuilds_incrementally() {
    let dir = std::env::temp_dir().join(format!("zustc_ws_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(Path::new("tests/workspace"), &dir);
    let rs: PathBuf = dir.join("target_rustc");
    std::fs::create_dir_all(&rs).unwrap();
    let st = Command::new("bash").arg(dir.join("build_rustc.sh")).arg(&rs).status().unwrap();
    assert!(st.success());
    let crates = zustc::session::read_workspace(&dir.join("workspace.txt")).unwrap();
    let out = dir.join("target");
    let mut s = zustc::session::Session::new(zustc::session::Options::default());
    let r = s.build_workspace(&crates, &out).unwrap();
    assert_eq!(r.linked, 1);
    // (by the number of arguments: 0 = division by 0 inside a dependency crate, 1 = normal, 2 = an index in a file of a bin's module, 3 = unwrap)
    for n in [0, 1, 2, 3, 5] {
        let a = run(&out.join("app"), &dir, n);
        let b = run(&rs.join("app"), &dir, n);
        assert_eq!(a, b, "args {n}");
    }
    // Without any change, it does nothing
    let r = s.build_workspace(&crates, &out).unwrap();
    assert_eq!((r.objects_written, r.linked), (0, 0));
    assert!(r.crates.iter().all(|(_, c)| c.fns_compiled == 0 && c.fns_hit == 0));
    // Change only a fn body in util: only util is recompiled (geom / app are not looked at since the interface is the same)
    let lib = dir.join("util/src/lib.rs");
    let t = std::fs::read_to_string(&lib).unwrap().replace("x * 2", "x * 3");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let compiled: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(compiled, ["util"]);
    assert_eq!(r.linked, 1);
    let (o, _, _) = run(&out.join("app"), &dir, 1);
    assert!(o.starts_with("(9, -12)"), "{o}");
    // Changing a signature recompiles only its users (geom is not looked at since it does not use calls)
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn calls() -> u32 { unsafe { CALLS } }", "pub fn calls() -> u64 { unsafe { CALLS as u64 } }");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let rebuilt: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled + c.fns_hit > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(rebuilt, ["util", "app"]);
    let (o, _, _) = run(&out.join("app"), &dir, 1);
    assert!(o.contains("calls 2"), "{o}");
    // Changing only a body: only util
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn helper(x: i64) -> i64 { unsafe { CALLS += 1; } x * 3 }", "pub fn helper(x: i64) -> i64 { unsafe { CALLS += 2; } x * 3 }");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let compiled: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(compiled, ["util"]);
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn helper(x: i64) -> i64 {", "pub fn helper(x: i64) -> i64 { let _ = 0u8;");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let rebuilt: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled + c.fns_hit > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(rebuilt, ["util"], "body change only");
    // Changing the signature of a fn that geom uses recompiles geom too (the same behavior as the one built with rustc)
    let geom = dir.join("geom/src/lib.rs");
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn helper(x: i64) -> i64 { let _ = 0u8; unsafe { CALLS += 2; } x * 3 }", "pub fn helper(x: i32) -> i64 { unsafe { CALLS += 2; } x as i64 * 3 }");
    std::fs::write(&lib, t).unwrap();
    let g = std::fs::read_to_string(&geom).unwrap().replace("P::new(util::helper(self.x), util::helper(self.y))", "P::new(util::helper(self.x as i32), util::helper(self.y as i32))");
    std::fs::write(&geom, g).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let rebuilt: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled + c.fns_hit > 0).map(|(n, _)| n.as_str()).collect();
    assert!(rebuilt.contains(&"util") && rebuilt.contains(&"geom"), "{rebuilt:?}");
    let (a, _, _) = run(&out.join("app"), &dir, 1);
    let rs2 = dir.join("target_rustc2");
    std::fs::create_dir_all(&rs2).unwrap();
    let st = Command::new("bash").arg(dir.join("build_rustc.sh")).arg(&rs2).status().unwrap();
    assert!(st.success());
    let (b, _, _) = run(&rs2.join("app"), &dir, 1);
    assert_eq!(a, b);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An executable whose name moves between bin crates (`exe=`, as zurgo writes it) is linked again when the crate it
/// comes from changes, though that crate's object is fresh and the file exists
#[test]
fn exe_moved_between_crates_relinks() {
    let dir = std::env::temp_dir().join(format!("zustc_ws_exe_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), "fn main() {\n    println!(\"a\");\n}\n").unwrap();
    std::fs::write(dir.join("b.rs"), "fn main() {\n    println!(\"b\");\n}\n").unwrap();
    let out = dir.join("t");
    let spec = |name: &str| zustc::session::CrateSpec {
        name: name.to_string(),
        root: dir.join(format!("{name}.rs")),
        display: format!("{name}.rs"),
        lib: false,
        deps: vec![],
        aliases: vec![],
        features: vec![],
        exe: Some("one".to_string()),
        cfgs: vec![],
        env: vec![],
        proc_macro: None,
        test: false,
    };
    let mut s = zustc::session::Session::new(zustc::session::Options::default());
    for (name, want) in [("a", "a\n"), ("b", "b\n"), ("a", "a\n")] {
        s.build_workspace(&[spec(name)], &out).unwrap();
        let (got, _, code) = run(&out.join("one"), &dir, 0);
        assert_eq!((got.as_str(), code), (want, Some(0)), "after building `{name}` as `one`");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A crate's own cfgs and compile-time environment (`cfg=` / `env=`, what a build script and cargo give): `cfg!`,
/// `#[cfg]`, `env!`, `option_env!` and `include!(concat!(env!(..), ..))` see them, and a change rebuilds the crate
#[test]
fn crate_cfgs_and_env() {
    let dir = std::env::temp_dir().join(format!("zustc_ws_env_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let put = |p: &str, t: &str| {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    };
    put("out/gen.rs", "pub fn gen() -> &'static str { \"generated\" }\npub fn gen_name() -> &'static str { env!(\"CARGO_PKG_NAME\") }\n");
    put(
        "lib/lib.rs",
        "include!(concat!(env!(\"OUT_DIR\"), \"/gen.rs\"));\npub const RAW: &str = r#\"env!(\"CARGO_PKG_NAME\")\"#;\n#[cfg(big)]\npub const SIZE: &str = \"big\";\n#[cfg(not(big))]\npub const SIZE: &str = \"small\";\npub fn hi() -> String {\n    let mut s = String::from(env!(\"CARGO_PKG_NAME\"));\n    if cfg!(kind = \"x y\") {\n        s.push_str(\" kind\");\n    }\n    s\n}\n",
    );
    put(
        "m/lib.rs",
        "macro_rules! big {\n    () => {\n        cfg!(big)\n    };\n}\nmacro_rules! label {\n    ($k:literal) => {\n        env!($k)\n    };\n}\npub fn is_big() -> bool {\n    big!()\n}\npub fn big_of<T>() -> bool {\n    big!()\n}\npub fn label() -> &'static str {\n    label!(\"M_LABEL\")\n}\npub fn spaced() -> &'static str {\n    env ! (\"M_LABEL\")\n}\n",
    );
    put(
        "app/main.rs",
        "fn main() {\n    println!(\"{} {} {} {:?} {:?}\", lib::hi(), lib::SIZE, lib::gen(), option_env!(\"APP_NOTE\"), option_env!(\"ZUST_SURELY_UNSET\"));\n    println!(\"{} {} {}\", lib::gen_name(), lib::RAW, m::is_big());\n    println!(\"{} {} {}\", m::big_of::<u8>(), m::label(), m::spaced());\n}\n",
    );
    let ws = |big: bool, m_big: bool| {
        let out = zustc::session::ws_escape(dir.join("out").to_str().unwrap());
        let cfg = if big { " cfg=big" } else { "" };
        let m_cfg = if m_big { " cfg=big" } else { "" };
        format!(
            "lib lib lib/lib.rs cfg=kind={}{cfg} env=OUT_DIR={out} env=CARGO_PKG_NAME=my%20lib\nlib m m/lib.rs{m_cfg} env=M_LABEL=m%20label\nbin app app/main.rs lib m env=APP_NOTE=a%23b\n",
            zustc::session::ws_escape("x y")
        )
    };
    let target = dir.join("target");
    // (`env!` in the included file; text in a raw string left alone; a cfg only a macro's body reads: a change of it
    //  alone rebuilds the crate, and the crate whose code a generic function of it is compiled in; `env!($k)` in a
    //  macro's body; `env !`)
    let raw = "env!(\"CARGO_PKG_NAME\")";
    // (`option_env!` of a name nothing sets: None, unless the runner's own environment has it)
    let unset = std::env::var("ZUST_SURELY_UNSET").map(|v| format!("Some({v:?})")).unwrap_or_else(|_| "None".into());
    for (big, m_big, want) in [
        (true, true, format!("my lib kind big generated Some(\"a#b\") {unset}\nmy lib {raw} true\ntrue m label m label\n")),
        (false, false, format!("my lib kind small generated Some(\"a#b\") {unset}\nmy lib {raw} false\nfalse m label m label\n")),
        // (only m's cfg: nothing of the app's text or of m's changes)
        (false, true, format!("my lib kind small generated Some(\"a#b\") {unset}\nmy lib {raw} true\ntrue m label m label\n")),
    ] {
        put("ws.txt", &ws(big, m_big));
        let crates = zustc::session::read_workspace(&dir.join("ws.txt")).unwrap();
        // (no forked workers: the other tests of this file run in threads beside this one)
        let mut s = zustc::session::Session::new(zustc::session::Options { jobs: 1, ..Default::default() });
        s.build_workspace(&crates, &target).unwrap();
        let (out, err, code) = run(&target.join("app"), &dir, 0);
        assert_eq!((out.as_str(), code), (want.as_str(), Some(0)), "{err}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What serde_core's lib.rs does with its modules: a `#[macro_use] mod` whose macros the rest of the root calls (it
/// goes where it is declared), and a macro that declares `pub mod de;` (de.rs beside the file that calls it).
/// `zustc check` reports a line of the root as the root's own line, past the module put in its place; and edition
/// 2021's reserved prefixes (`demo!(foo#bar)`) are an error before the macro reads its input
#[test]
fn macro_use_modules_and_check_lines() {
    let dir = std::env::temp_dir().join(format!("zustc_ws_macro_use_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let put = |p: &str, t: &str| std::fs::write(dir.join(p), t).unwrap();
    put("crate_root.rs", "// 😀 mod de; (a comment: no declaration)\n#[cfg(any())]\nmod de;\nmacro_rules! crate_root [\n    () => {\n        pub mod de;\n    };\n];\nmacro_rules! twice {\n    ($e:expr) => {\n        $e * 2\n    };\n}\n");
    put("de.rs", "pub fn name() -> &'static str {\n    \"de\"\n}\n");
    put("main.rs", "#[macro_use]\nmod crate_root;\n\ncrate_root!();\n\nfn main() {\n    println!(\"{} {}\", twice!(4), de::name());\n}\n");
    let zustc = env!("CARGO_BIN_EXE_zustc");
    let exe = dir.join("main");
    let o = Command::new(zustc).arg("--no-cache").arg(dir.join("main.rs")).arg("-o").arg(&exe).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let (got, _, code) = run(&exe, &dir, 0);
    assert_eq!((got.as_str(), code), ("8 de\n", Some(0)));
    // (the error on line 7 of main.rs, past the lines of crate_root.rs inlined at line 2)
    put("main.rs", "#[macro_use]\nmod crate_root;\n\ncrate_root!();\n\nfn main() {\n    let x: u8 = \"s\";\n    println!(\"{} {} {x}\", twice!(4), de::name());\n}\n");
    let o = Command::new(zustc).arg("check").arg(dir.join("main.rs")).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("main.rs:7:17: mismatched types"), "{err}");
    put("p.rs", "macro_rules! demo {\n    ($a:tt $b:tt $c:tt) => {};\n}\nfn main() {\n    demo!(foo#bar);\n}\n");
    let o = Command::new(zustc).arg("check").arg(dir.join("p.rs")).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains(":5:11: prefix `foo` is unknown"), "{err}");
    let o = Command::new(zustc).arg("check").arg("--edition=2018").arg(dir.join("p.rs")).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // (a raw identifier before `#`: `r#let#foo` is three tokens in 2021 too)
    put("q.rs", "macro_rules! demo {\n    ($a:tt $b:tt $c:tt) => {};\n}\nfn main() {\n    demo!(r#let#foo);\n}\n");
    let o = Command::new(zustc).arg("check").arg(dir.join("q.rs")).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // (`#[macro_use] #[path = ..] mod`: the path beside the declaring file, the module where it is declared)
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    put("sub/outer.rs", "#[macro_use]\n#[path = \"helpers.rs\"]\nmod helpers;\npub fn four() -> u8 {\n    two!() * 2\n}\n");
    put("sub/helpers.rs", "macro_rules! two {\n    () => {\n        2\n    };\n}\n");
    put("r.rs", "#[path = \"sub/outer.rs\"]\nmod outer;\nfn main() {\n    println!(\"{}\", outer::four());\n}\n");
    let exe = dir.join("r");
    let o = Command::new(zustc).arg("--no-cache").arg(dir.join("r.rs")).arg("-o").arg(&exe).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(run(&exe, &dir, 0).0, "4\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// unused_parens (#175) in a workspace: a library's own `#![deny(unused_parens)]` is followed (rustc: lib.rs:3:13),
/// and a binary's is not applied to the code of the crates it depends on (rustc builds it: the dependency only warns)
#[test]
fn unused_parens_in_a_library_and_not_in_dependencies() {
    let dir = std::env::temp_dir().join(format!("zustc_ws_parens_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let put = |p: &str, t: &str| std::fs::write(dir.join(p), t).unwrap();
    put("lib.rs", "#![deny(unused_parens)]\npub fn f() -> i32 {\n    let x = (1);\n    x\n}\n");
    put("dep.rs", "pub fn g() -> i32 {\n    let x = (2);\n    x\n}\n");
    put("main.rs", "#![deny(unused_parens)]\nfn main() {\n    println!(\"{}\", dep::g());\n}\n");
    let spec = |name: &str, lib: bool, deps: &[&str]| zustc::session::CrateSpec {
        name: name.to_string(),
        root: dir.join(format!("{name}.rs")),
        display: format!("{name}.rs"),
        lib,
        deps: deps.iter().map(|d| d.to_string()).collect(),
        aliases: vec![],
        features: vec![],
        exe: (!lib).then(|| name.to_string()),
        cfgs: vec![],
        env: vec![],
        proc_macro: None,
        test: false,
    };
    let out = dir.join("t");
    let mut s = zustc::session::Session::new(zustc::session::Options::default());
    let e = s.build_workspace(&[spec("lib", true, &[])], &out).err().unwrap_or_default();
    assert!(e.contains("lib.rs:3:13: unnecessary parentheses around assigned value"), "{e}");
    s.build_workspace(&[spec("dep", true, &[]), spec("main", false, &["dep"])], &out).unwrap();
    assert_eq!(run(&out.join("main"), &dir, 0).0, "2\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Renamed dependencies (`extern=ALIAS=NAME`; a Cargo `common = { package = "crypto-common" }`, rustc's `--extern
/// common=..`): the dependent names the library otherwise in each of its modules (a fn, a type, a trait, `use
/// alias::..` of a module's item and of an item, `::alias::..`, in a module of another file), re-exports it (`pub use
/// common;`) and names it in `extern crate` (`extern crate lock_api_crate as la;`); a module of the library's own name
/// beside the renamed library (spin's `lock_api` with `lock_api_crate`); a bin's names, from a module of its own. The
/// output is rustc's
#[test]
fn renamed_dependencies_match_rustc() {
    let dir = std::env::temp_dir().join(format!("zustc_ws_renamed_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let put = |p: &str, t: &str| {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    };
    put(
        "cc/lib.rs",
        "pub mod hazmat;\npub struct Block(pub u32);\npub fn make(x: u32) -> Block {\n    Block(x * 2)\n}\npub mod inner {\n    pub fn hello() -> &'static str {\n        \"hello\"\n    }\n}\npub trait Tr {\n    fn get(&self) -> u32;\n}\nimpl Tr for Block {\n    fn get(&self) -> u32 {\n        self.0 + 1\n    }\n}\n",
    );
    put("cc/hazmat.rs", "pub struct State<T>(pub T);\npub fn state<T>(t: T) -> State<T> {\n    State(t)\n}\n");
    put(
        "lock_api/lib.rs",
        "pub unsafe trait RawMutex {\n    const INIT: Self;\n    fn lock(&self) -> u32;\n}\npub struct Mutex<R: RawMutex, T> {\n    pub raw: R,\n    pub data: T,\n}\nimpl<R: RawMutex, T: Copy> Mutex<R, T> {\n    pub fn get(&self) -> (u32, T) {\n        (self.raw.lock(), self.data)\n    }\n}\n",
    );
    put(
        "dig/lib.rs",
        "extern crate lock_api_crate as la;\npub use common;\npub use common::Block;\nmod sub;\n// (a module of the library's name: `lock_api` is this one, `lock_api_crate` the library)\npub mod lock_api {\n    pub type Mutex<T> = lock_api_crate::Mutex<crate::Raw, T>;\n    pub fn new<T>(data: T) -> Mutex<T> {\n        lock_api_crate::Mutex { raw: <crate::Raw as lock_api_crate::RawMutex>::INIT, data }\n    }\n}\npub struct Raw(pub u32);\nunsafe impl la::RawMutex for Raw {\n    const INIT: Self = Raw(100);\n    fn lock(&self) -> u32 {\n        self.0 + 1\n    }\n}\npub fn all() -> String {\n    let m = lock_api::new(7u8);\n    format!(\"{} {} {:?}\", sub::f(), common::make(1).0, m.get())\n}\n",
    );
    put(
        "dig/sub.rs",
        "use common::hazmat::State;\nuse common::inner::hello;\nuse common::Tr;\npub fn f() -> String {\n    let s: State<&str> = common::hazmat::state(\"st\");\n    let b: ::common::Block = common::make(3);\n    format!(\"{} {} {}\", hello(), b.get(), s.0)\n}\n",
    );
    put(
        "app/main.rs",
        "mod show {\n    use cc::Tr;\n    pub fn line() -> String {\n        let b: d::Block = cc::Block(9);\n        format!(\"{} {} {} {}\", d::all(), cc::make(5).0, d::common::inner::hello(), b.get())\n    }\n}\nfn main() {\n    println!(\"{}\", show::line());\n}\n",
    );
    put(
        "ws.txt",
        "lib crypto_common cc/lib.rs\nlib lock_api lock_api/lib.rs\nlib dig dig/lib.rs extern=common=crypto_common extern=lock_api_crate=lock_api\nbin app app/main.rs extern=d=dig extern=cc=crypto_common\n",
    );
    let crates = zustc::session::read_workspace(&dir.join("ws.txt")).unwrap();
    assert_eq!(crates[2].deps, ["crypto_common", "lock_api"]);
    let alias = |a: &str, n: &str| (a.to_string(), n.to_string());
    assert_eq!(crates[2].aliases, [alias("common", "crypto_common"), alias("lock_api_crate", "lock_api")]);
    let out = dir.join("target");
    let mut s = zustc::session::Session::new(zustc::session::Options { jobs: 1, ..Default::default() });
    s.build_workspace(&crates, &out).unwrap();
    let rs = dir.join("target_rustc");
    rustc_build(&crates, &rs);
    let a = run(&out.join("app"), &dir, 0);
    assert_eq!(a, run(&rs.join("app"), &dir, 0));
    assert_eq!(a.0, "hello 7 st 2 (101, 7) 10 hello 10\n");
    // (`extern=` with no crate)
    put("bad.txt", "lib crypto_common cc/lib.rs\nlib dig dig/lib.rs extern=common\n");
    let e = zustc::session::read_workspace(&dir.join("bad.txt")).err().unwrap_or_default();
    assert!(e.contains("expected `extern=ALIAS=NAME`"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A workspace's crates built by rustc into rs, as zustc builds them: `--extern` for each dependency, a renamed one
/// under its name (`--extern common=libcrypto_common.rlib`)
fn rustc_build(crates: &[zustc::session::CrateSpec], rs: &Path) {
    std::fs::create_dir_all(rs).unwrap();
    let rlib = |n: &str| rs.join(format!("lib{n}.rlib"));
    for c in crates {
        let mut cmd = Command::new("rustc");
        cmd.args(["--edition", "2021", "-Awarnings", "--crate-name", &c.name]).arg(&c.root).arg("-L").arg(rs);
        if c.lib {
            cmd.args(["--crate-type", "rlib"]).arg("-o").arg(rlib(&c.name));
        } else {
            cmd.arg("-o").arg(rs.join(&c.name));
        }
        for d in c.deps.iter().filter(|d| !c.aliases.iter().any(|x| x.1 == **d)) {
            cmd.arg("--extern").arg(format!("{d}={}", rlib(d).display()));
        }
        for (a, n) in &c.aliases {
            cmd.arg("--extern").arg(format!("{a}={}", rlib(n).display()));
        }
        let o = cmd.output().unwrap();
        assert!(o.status.success(), "rustc {}: {}", c.name, String::from_utf8_lossy(&o.stderr));
    }
}

/// The files (with `ws.txt`) built by zustc and by rustc: what the bin `app` prints in each
fn zustc_and_rustc(name: &str, files: &[(&str, &str)]) -> (String, String) {
    let dir = std::env::temp_dir().join(format!("zustc_ws_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (p, t) in files {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
    let crates = zustc::session::read_workspace(&dir.join("ws.txt")).unwrap();
    let out = dir.join("target");
    let mut s = zustc::session::Session::new(zustc::session::Options { jobs: 1, ..Default::default() });
    let built = s.build_workspace(&crates, &out);
    rustc_build(&crates, &dir.join("target_rustc"));
    let want = run(&dir.join("target_rustc/app"), &dir, 0).0;
    let got = match built {
        Ok(_) => run(&out.join("app"), &dir, 0).0,
        Err(e) => format!("zustc: {e}"),
    };
    let _ = std::fs::remove_dir_all(&dir);
    (got, want)
}

/// `::common::..` is the renamed dependency past a module `common` of the crate's own (edition 2018 on): in a `use` of an
/// item, of the crate itself (`as c`) and of its module (`as i`), and in an expression; `common::..` is the module
#[test]
fn renamed_dependency_absolute_paths_past_a_module_of_its_name() {
    let (got, want) = zustc_and_rustc(
        "renamed_abs",
        &[
            ("cc.rs", "pub fn make() -> u32 {\n    1\n}\npub mod inner {\n    pub fn hi() -> u32 {\n        3\n    }\n}\n"),
            (
                "dig.rs",
                "mod common {\n    pub fn make() -> u32 {\n        2\n    }\n}\nuse ::common::make;\nuse ::common as c;\nuse ::common::inner as i;\npub fn f() -> u32 {\n    i::hi() * 10000 + make() * 1000 + ::common::make() * 100 + c::make() * 10 + common::make()\n}\n",
            ),
            ("app.rs", "fn main() {\n    println!(\"{}\", dig::f());\n}\n"),
            ("ws.txt", "lib cc cc.rs\nlib dig dig.rs extern=common=cc\nbin app app.rs dig\n"),
        ],
    );
    assert_eq!(want, "31112\n");
    assert_eq!(got, want);
}

/// A fn of the renamed dependency's name does not hide it: the fn is a value, `common::..` a path from a type or module
#[test]
fn renamed_dependency_beside_a_fn_of_its_name() {
    let (got, want) = zustc_and_rustc(
        "renamed_fn",
        &[
            ("cc.rs", "pub fn make() -> u32 {\n    1\n}\n"),
            ("dig.rs", "fn common() -> u32 {\n    5\n}\npub fn f() -> u32 {\n    common::make() * 10 + common()\n}\n"),
            ("app.rs", "fn main() {\n    println!(\"{}\", dig::f());\n}\n"),
            ("ws.txt", "lib cc cc.rs\nlib dig dig.rs extern=common=cc\nbin app app.rs dig\n"),
        ],
    );
    assert_eq!(want, "15\n");
    assert_eq!(got, want);
}

/// A module of the renamed dependency's name shadows it where it is in scope, also for a path through the module's
/// re-export (`common::inner` with `pub use crate::local::inner;` in it); a module beneath does not see it
#[test]
fn renamed_dependency_shadowed_by_a_module_that_reexports() {
    let (got, want) = zustc_and_rustc(
        "renamed_shadow",
        &[
            ("cc.rs", "pub mod inner {\n    pub fn make() -> u32 {\n        1\n    }\n}\n"),
            (
                "dig.rs",
                "mod local {\n    pub mod inner {\n        pub fn make() -> u32 {\n            2\n        }\n    }\n}\nmod common {\n    pub use crate::local::inner;\n}\nmod sub {\n    pub fn g() -> u32 {\n        common::inner::make()\n    }\n}\npub fn f() -> u32 {\n    common::inner::make() * 10 + sub::g()\n}\n",
            ),
            ("app.rs", "fn main() {\n    println!(\"{}\", dig::f());\n}\n"),
            ("ws.txt", "lib cc cc.rs\nlib dig dig.rs extern=common=cc\nbin app app.rs dig\n"),
        ],
    );
    assert_eq!(want, "21\n");
    assert_eq!(got, want);
}

/// A name another crate gives a dependency is that crate's alone: the bin names `other` `common`, and a generic fn of
/// `dig`, which depends on the crate `common` and which the bin instantiates, still means `common` by `::common::..` and
/// by `common::..`
#[test]
fn renamed_dependency_of_a_bin_does_not_reach_a_library() {
    let (got, want) = zustc_and_rustc(
        "renamed_leak",
        &[
            ("common.rs", "pub fn make() -> u32 {\n    1\n}\n"),
            ("other.rs", "pub fn make() -> u32 {\n    2\n}\n"),
            ("dig.rs", "pub fn f<T>() -> u32 {\n    ::common::make() * 10 + common::make()\n}\n"),
            ("app.rs", "fn main() {\n    println!(\"{} {}\", dig::f::<u8>(), common::make());\n}\n"),
            ("ws.txt", "lib common common.rs\nlib other other.rs\nlib dig dig.rs common\nbin app app.rs dig extern=common=other\n"),
        ],
    );
    assert_eq!(want, "11 2\n");
    assert_eq!(got, want);
}

/// Only a dependency's renamed dependency changes (`chosen=a` to `chosen=b`, the texts the same): its generic fn, which
/// the bin instantiates, names the other crate now, so the bin is compiled again though the items it reaches are the same
#[test]
fn a_dependency_renaming_another_crate_recompiles_its_users() {
    let dir = std::env::temp_dir().join(format!("zustc_ws_renamed_fp_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let put = |p: &str, t: &str| std::fs::write(dir.join(p), t).unwrap();
    put("a.rs", "pub fn val() -> u32 {\n    1\n}\n");
    put("b.rs", "pub fn val() -> u32 {\n    2\n}\n");
    put("middle.rs", "pub fn get<T>() -> u32 {\n    chosen::val()\n}\n");
    put("app.rs", "fn main() {\n    println!(\"{}\", middle::get::<u8>());\n}\n");
    let out = dir.join("target");
    for (alias, want) in [("a", "1\n"), ("b", "2\n"), ("a", "1\n")] {
        put("ws.txt", &format!("lib a a.rs\nlib b b.rs\nlib middle middle.rs a b extern=chosen={alias}\nbin app app.rs middle\n"));
        let crates = zustc::session::read_workspace(&dir.join("ws.txt")).unwrap();
        let mut s = zustc::session::Session::new(zustc::session::Options { jobs: 1, ..Default::default() });
        s.build_workspace(&crates, &out).unwrap();
        assert_eq!(run(&out.join("app"), &dir, 0).0, want, "chosen={alias}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
