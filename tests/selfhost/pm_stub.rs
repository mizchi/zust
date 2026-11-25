// crates/syntax/src/pm.rs の代わり (セルフホスト): proc macro は toolchain の proc-macro server (子プロセス) が
// 展開するが、zustc の std には std::process::Child がなく、セルフホストのコンパイラに proc-macro クレートを渡すもの
// (zurgo) もない。型と関数の形だけを同じにし、ライブラリは来ない (Scope は作られず、expand_items は何も見つけない)。
// rustc 版と zustc 版の両方がこれを使うので、2 つのコンパイラの出力は比べられる

use std::path::{Path, PathBuf};

pub const MARK: &str = crate::lexer::PM_MARK;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Attr,
    Derive,
    Bang,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lib {
    pub name: String,
    pub dylib: PathBuf,
    pub macros: Vec<(String, Kind)>,
    pub via: Vec<(String, String, String)>,
}

const NONE: &str = "proc macros are not run by the self-hosted compiler";

pub fn load(_name: &str, _dylib: &Path) -> Result<Lib, String> {
    Err(NONE.to_string())
}

pub fn expand(_lib: &Lib, _name: &str, _attr: Option<&str>, _body: &str, _env: &[(String, String)]) -> Result<String, String> {
    Err(NONE.to_string())
}

pub struct Scope<'a> {
    libs: &'a [Lib],
    names: Vec<String>,
}

impl<'a> Scope<'a> {
    pub fn new(libs: &'a [Lib], _b: &[u8], _toks: &[(u8, usize, usize)]) -> Scope<'a> {
        Scope { libs: libs, names: Vec::new() }
    }
    pub fn names_crate(&self, _segs: &[&str]) -> bool {
        false
    }
    pub fn aliases(&self) -> std::slice::Iter<'_, String> {
        self.names.iter()
    }
    pub fn resolve(&self, _segs: &[&str], _kind: Kind) -> Option<(&'a Lib, String)> {
        let _ = self.libs;
        None
    }
}

pub fn pub_uses(_text: &str) -> Vec<(String, String, String)> {
    Vec::new()
}

pub fn note_reexports(_libs: &mut [Lib], _lib: &str, _uses: &[(String, String, String)]) {}

pub fn expand_items(_text: &str, _libs: &[Lib], _env: &[(String, String)]) -> Result<Vec<(usize, usize, String)>, String> {
    Ok(Vec::new())
}
