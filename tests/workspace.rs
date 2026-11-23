//! 複数クレート・複数ファイルのワークスペース (tests/workspace) を `Session2::build_workspace` でビルドし、
//! rustc (rlib + リンク) で作ったものと出力・終了コード・panic のメッセージ (クレートとファイルをまたぐ場所) が同じか。
//! さらに、依存されるクレートの関数の本体だけを変えると、そのクレートだけをコンパイルし直すか
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
    // (新しい rustc はスレッドの ID を出す: `thread 'main' (1234) panicked`)
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
    let crates = zustc::driver2::read_workspace(&dir.join("workspace.txt")).unwrap();
    let out = dir.join("target");
    let mut s = zustc::driver2::Session2::new(zustc::driver2::Options2::default());
    let r = s.build_workspace(&crates, &out).unwrap();
    assert_eq!(r.linked, 1);
    // (引数の数で: 0 = 依存するクレートの中の 0 除算, 1 = 普通, 2 = bin のモジュールのファイルの添字, 3 = unwrap)
    for n in [0, 1, 2, 3, 5] {
        let a = run(&out.join("app"), &dir, n);
        let b = run(&rs.join("app"), &dir, n);
        assert_eq!(a, b, "args {n}");
    }
    // 何も変えなければ何もしない
    let r = s.build_workspace(&crates, &out).unwrap();
    assert_eq!((r.objects_written, r.linked), (0, 0));
    assert!(r.crates.iter().all(|(_, c)| c.fns_compiled == 0 && c.fns_hit == 0));
    // util の関数の本体だけを変える: util だけコンパイルし直す (geom・app はインターフェースが同じなので見ない)
    let lib = dir.join("util/src/lib.rs");
    let t = std::fs::read_to_string(&lib).unwrap().replace("x * 2", "x * 3");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let compiled: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(compiled, ["util"]);
    assert_eq!(r.linked, 1);
    let (o, _, _) = run(&out.join("app"), &dir, 1);
    assert!(o.starts_with("(9, -12)"), "{o}");
    // シグネチャを変えると、それを使う側だけコンパイルし直す (geom は calls を使わないので見ない)
    let t = std::fs::read_to_string(&lib).unwrap().replace("pub fn calls() -> u32 { unsafe { CALLS } }", "pub fn calls() -> u64 { unsafe { CALLS as u64 } }");
    std::fs::write(&lib, t).unwrap();
    let r = s.build_workspace(&crates, &out).unwrap();
    let rebuilt: Vec<&str> = r.crates.iter().filter(|(_, c)| c.fns_compiled + c.fns_hit > 0).map(|(n, _)| n.as_str()).collect();
    assert_eq!(rebuilt, ["util", "app"]);
    let (o, _, _) = run(&out.join("app"), &dir, 1);
    assert!(o.contains("calls 2"), "{o}");
    // 本体だけを変えれば util だけ
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
    // geom が使う関数のシグネチャを変えると geom も (rustc で作ったものと同じ動き)
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
    let spec = |name: &str| zustc::driver2::CrateSpec {
        name: name.to_string(),
        root: dir.join(format!("{name}.rs")),
        display: format!("{name}.rs"),
        lib: false,
        deps: vec![],
        features: vec![],
        exe: Some("one".to_string()),
        cfgs: vec![],
        env: vec![],
        proc_macro: None,
    };
    let mut s = zustc::driver2::Session2::new(zustc::driver2::Options2::default());
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
        let out = zustc::driver2::ws_escape(dir.join("out").to_str().unwrap());
        let cfg = if big { " cfg=big" } else { "" };
        let m_cfg = if m_big { " cfg=big" } else { "" };
        format!(
            "lib lib lib/lib.rs cfg=kind={}{cfg} env=OUT_DIR={out} env=CARGO_PKG_NAME=my%20lib\nlib m m/lib.rs{m_cfg} env=M_LABEL=m%20label\nbin app app/main.rs lib m env=APP_NOTE=a%23b\n",
            zustc::driver2::ws_escape("x y")
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
        let crates = zustc::driver2::read_workspace(&dir.join("ws.txt")).unwrap();
        // (no forked workers: the other tests of this file run in threads beside this one)
        let mut s = zustc::driver2::Session2::new(zustc::driver2::Options2 { jobs: 1, ..Default::default() });
        s.build_workspace(&crates, &target).unwrap();
        let (out, err, code) = run(&target.join("app"), &dir, 0);
        assert_eq!((out.as_str(), code), (want.as_str(), Some(0)), "{err}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
