//! Targets and `[target.'cfg(..)'.dependencies]`: a platform key is a target triple or a `cfg(..)` expression,
//! evaluated against the values `rustc --print cfg --target <triple>` prints (not zustc's own cfg set: zustc keeps
//! 64-bit pointers on wasm32, but which dependencies a package has is cargo's choice and must be cargo's).

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub triple: String,
    /// (`unix`, `target_os = "linux"`, ...)
    pub cfg: Vec<(String, Option<String>)>,
}

const COMMON_ATOMICS: [&str; 5] = ["8", "16", "32", "64", "ptr"];

impl Target {
    pub fn host() -> Target {
        Target::by_triple("x86_64-unknown-linux-gnu").unwrap()
    }

    pub fn by_triple(triple: &str) -> Option<Target> {
        let kv = |k: &str, v: &str| (k.to_string(), Some(v.to_string()));
        let mut cfg: Vec<(String, Option<String>)> = vec![("debug_assertions".into(), None), kv("target_abi", ""), kv("target_endian", "little"), kv("target_vendor", "unknown")];
        cfg.extend(COMMON_ATOMICS.iter().map(|a| kv("target_has_atomic", a)));
        match triple {
            "x86_64-unknown-linux-gnu" => {
                cfg.push(("unix".into(), None));
                cfg.extend([kv("panic", "unwind"), kv("target_arch", "x86_64"), kv("target_env", "gnu"), kv("target_family", "unix")]);
                cfg.extend(["fxsr", "sse", "sse2"].iter().map(|f| kv("target_feature", f)));
                cfg.extend([kv("target_os", "linux"), kv("target_pointer_width", "64")]);
            }
            "wasm32-unknown-unknown" => {
                cfg.extend([kv("panic", "abort"), kv("target_arch", "wasm32"), kv("target_env", ""), kv("target_family", "wasm")]);
                cfg.extend(
                    ["bulk-memory", "multivalue", "mutable-globals", "nontrapping-fptoint", "reference-types", "sign-ext"].iter().map(|f| kv("target_feature", f)),
                );
                cfg.extend([kv("target_os", "unknown"), kv("target_pointer_width", "32")]);
            }
            _ => return None,
        }
        Some(Target { triple: triple.to_string(), cfg })
    }

    fn has(&self, key: &str, val: Option<&str>) -> bool {
        self.cfg.iter().any(|(k, v)| k == key && v.as_deref() == val)
    }

    /// The value of a key (`target_arch`), for `CARGO_CFG_*`
    pub fn values(&self, key: &str) -> Vec<&str> {
        self.cfg.iter().filter(|(k, _)| k == key).filter_map(|(_, v)| v.as_deref()).collect()
    }

    /// Whether a `[target.<key>]` platform applies: a triple, or `cfg(..)`
    pub fn matches(&self, platform: &str) -> Result<bool, String> {
        let p = platform.trim();
        match p.strip_prefix("cfg(").and_then(|r| r.strip_suffix(')')) {
            Some(inner) => {
                let toks = tokenize(inner)?;
                let mut i = 0;
                let v = self.eval(&toks, &mut i)?;
                if i != toks.len() {
                    return Err(format!("bad cfg expression `{p}`"));
                }
                Ok(v)
            }
            None => Ok(p == self.triple),
        }
    }

    fn eval(&self, t: &[Tok], i: &mut usize) -> Result<bool, String> {
        let Some(Tok::Ident(name)) = t.get(*i) else { return Err("expected a cfg name".into()) };
        *i += 1;
        if matches!(name.as_str(), "all" | "any" | "not") && t.get(*i) == Some(&Tok::Open) {
            *i += 1;
            let mut vals = vec![];
            while t.get(*i) != Some(&Tok::Close) {
                vals.push(self.eval(t, i)?);
                match t.get(*i) {
                    Some(Tok::Comma) => *i += 1,
                    Some(Tok::Close) => {}
                    _ => return Err("expected `,` or `)` in a cfg expression".into()),
                }
            }
            *i += 1;
            return Ok(match name.as_str() {
                "all" => vals.iter().all(|&v| v),
                "any" => vals.iter().any(|&v| v),
                _ => {
                    if vals.len() != 1 {
                        return Err("`not` takes one predicate".into());
                    }
                    !vals[0]
                }
            });
        }
        if t.get(*i) == Some(&Tok::Eq) {
            let Some(Tok::Str(v)) = t.get(*i + 1) else { return Err("expected a string after `=`".into()) };
            *i += 2;
            return Ok(self.has(name, Some(v)));
        }
        Ok(match name.as_str() {
            "true" => true,
            "false" => false,
            _ => self.has(name, None),
        })
    }
}

/// A platform key as cargo prints it: `cfg(..)` re-written from its parse (`cfg(target_os="x")` →
/// `cfg(target_os = "x")`, `all(a,b)` → `all(a, b)`); a triple as it is
pub fn normalize(platform: &str) -> String {
    let p = platform.trim();
    let Some(inner) = p.strip_prefix("cfg(").and_then(|r| r.strip_suffix(')')) else { return p.to_string() };
    let Ok(toks) = tokenize(inner) else { return p.to_string() };
    let mut out = String::from("cfg(");
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Ident(n) => out.push_str(n),
            Tok::Str(v) => {
                out.push('"');
                out.push_str(v);
                out.push('"');
            }
            Tok::Open => out.push('('),
            Tok::Close => out.push(')'),
            Tok::Comma => {
                // (no trailing comma: `all(a, )` is `all(a)`)
                if toks.get(i + 1) != Some(&Tok::Close) {
                    out.push_str(", ");
                }
            }
            Tok::Eq => out.push_str(" = "),
        }
        i += 1;
    }
    out.push(')');
    out
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Open,
    Close,
    Comma,
    Eq,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'(' => {
                out.push(Tok::Open);
                i += 1;
            }
            b')' => {
                out.push(Tok::Close);
                i += 1;
            }
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            b'=' => {
                out.push(Tok::Eq);
                i += 1;
            }
            b'"' => {
                let a = i + 1;
                i = a;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                out.push(Tok::Str(s[a..i.min(b.len())].to_string()));
                i += 1;
            }
            _ if c.is_ascii_alphanumeric() || c == b'_' => {
                let a = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(Tok::Ident(s[a..i].to_string()));
            }
            _ => return Err(format!("unexpected `{}` in a cfg expression", c as char)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg_expressions() {
        let h = Target::host();
        let w = Target::by_triple("wasm32-unknown-unknown").unwrap();
        for (p, on_host, on_wasm) in [
            ("cfg(unix)", true, false),
            ("cfg(windows)", false, false),
            ("cfg(target_arch = \"wasm32\")", false, true),
            ("cfg(all(target_arch = \"wasm32\", target_os = \"unknown\"))", false, true),
            ("cfg(any(target_os = \"linux\", target_os = \"android\"))", true, false),
            ("cfg(not(any(target_arch = \"wasm32\", windows)))", true, false),
            ("cfg(target_pointer_width = \"32\")", false, true),
            ("cfg(target_feature = \"reference-types\")", false, true),
            ("x86_64-unknown-linux-gnu", true, false),
            ("x86_64-pc-windows-msvc", false, false),
        ] {
            assert_eq!(h.matches(p), Ok(on_host), "{p} on the host");
            assert_eq!(w.matches(p), Ok(on_wasm), "{p} on wasm32");
        }
        assert!(h.matches("cfg(all(unix,)").is_err());
        assert_eq!(normalize("cfg(target_os=\"hermit\")"), "cfg(target_os = \"hermit\")");
        assert_eq!(normalize("cfg(all(unix,not( windows )))"), "cfg(all(unix, not(windows)))");
        assert_eq!(normalize("x86_64-pc-windows-gnu"), "x86_64-pc-windows-gnu");
    }
}
