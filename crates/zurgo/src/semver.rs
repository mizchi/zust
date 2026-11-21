//! Versions and version requirements, with Cargo's rules (`^` by default, `~`, `=`, `<`, `<=`, `>`, `>=`,
//! wildcards, pre-releases matching only a comparator of the same `major.minor.patch`).

use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// (`-alpha.1`: the identifiers; empty for a release)
    pub pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(str::to_string).collect()),
            None => (s, vec![]),
        };
        let mut it = core.split('.');
        let v = Version { major: it.next()?.parse().ok()?, minor: it.next()?.parse().ok()?, patch: it.next()?.parse().ok()?, pre };
        if it.next().is_some() { None } else { Some(v) }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

fn cmp_pre(a: &[String], b: &[String]) -> Ordering {
    // (a release is above its pre-releases)
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    for (x, y) in a.iter().zip(b) {
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(m), Ok(n)) => m.cmp(&n),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            _ => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(o.major, o.minor, o.patch)).then_with(|| cmp_pre(&self.pre, &o.pre))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Exact,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Tilde,
    Caret,
    Wildcard,
}

#[derive(Clone, Debug, PartialEq)]
struct Comparator {
    op: Op,
    major: u64,
    minor: Option<u64>,
    patch: Option<u64>,
    pre: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VersionReq {
    cmps: Vec<Comparator>,
}

impl VersionReq {
    pub fn parse(s: &str) -> Option<VersionReq> {
        let s = s.trim();
        if s.is_empty() || s == "*" {
            return Some(VersionReq { cmps: vec![] });
        }
        let mut cmps = vec![];
        for part in s.split(',') {
            let p = part.trim();
            let (op, rest) = [(">=", Op::GreaterEq), ("<=", Op::LessEq), (">", Op::Greater), ("<", Op::Less), ("=", Op::Exact), ("~", Op::Tilde), ("^", Op::Caret)]
                .iter()
                .find(|(t, _)| p.starts_with(t))
                .map(|(t, o)| (*o, p[t.len()..].trim()))
                .unwrap_or((Op::Caret, p));
            if rest == "*" {
                continue;
            }
            let (core, pre) = match rest.split('+').next()?.split_once('-') {
                Some((c, pr)) => (c, pr.split('.').map(str::to_string).collect()),
                None => (rest.split('+').next()?, vec![]),
            };
            let mut it = core.split('.');
            let major: u64 = it.next()?.parse().ok()?;
            let mut wild = false;
            let mut num = |x: Option<&str>| -> Option<Option<u64>> {
                match x {
                    None => Some(None),
                    Some("*" | "x" | "X") => {
                        wild = true;
                        Some(None)
                    }
                    Some(n) => n.parse().ok().map(Some),
                }
            };
            let minor = num(it.next())?;
            let patch = num(it.next())?;
            let op = if wild && op == Op::Caret { Op::Wildcard } else { op };
            cmps.push(Comparator { op, major, minor, patch, pre });
        }
        Some(VersionReq { cmps })
    }

    pub fn matches(&self, v: &Version) -> bool {
        if !self.cmps.iter().all(|c| c.matches(v)) {
            return false;
        }
        // (a pre-release only if a comparator names the same version with a pre-release)
        v.pre.is_empty()
            || self.cmps.iter().any(|c| c.major == v.major && c.minor == Some(v.minor) && c.patch == Some(v.patch) && !c.pre.is_empty())
    }
}

/// (as the semver crate's `eval.rs`: components are compared as given, so a partial bound such as `<1.2` covers the
/// whole `1.2` line, and nothing is incremented (a component may be `u64::MAX`))
impl Comparator {
    fn matches(&self, v: &Version) -> bool {
        match self.op {
            Op::Exact | Op::Wildcard => self.exact(v),
            Op::Greater => self.greater(v),
            Op::GreaterEq => self.exact(v) || self.greater(v),
            Op::Less => self.less(v),
            Op::LessEq => self.exact(v) || self.less(v),
            Op::Tilde => self.tilde(v),
            Op::Caret => self.caret(v),
        }
    }

    fn exact(&self, v: &Version) -> bool {
        v.major == self.major && self.minor.is_none_or(|m| v.minor == m) && self.patch.is_none_or(|p| v.patch == p) && v.pre == self.pre
    }

    fn greater(&self, v: &Version) -> bool {
        if v.major != self.major {
            return v.major > self.major;
        }
        let Some(m) = self.minor else { return false };
        if v.minor != m {
            return v.minor > m;
        }
        let Some(p) = self.patch else { return false };
        if v.patch != p {
            return v.patch > p;
        }
        cmp_pre(&v.pre, &self.pre) == Ordering::Greater
    }

    fn less(&self, v: &Version) -> bool {
        if v.major != self.major {
            return v.major < self.major;
        }
        let Some(m) = self.minor else { return false };
        if v.minor != m {
            return v.minor < m;
        }
        let Some(p) = self.patch else { return false };
        if v.patch != p {
            return v.patch < p;
        }
        cmp_pre(&v.pre, &self.pre) == Ordering::Less
    }

    fn tilde(&self, v: &Version) -> bool {
        if v.major != self.major || self.minor.is_some_and(|m| v.minor != m) {
            return false;
        }
        if let Some(p) = self.patch {
            if v.patch != p {
                return v.patch > p;
            }
        }
        cmp_pre(&v.pre, &self.pre) != Ordering::Less
    }

    fn caret(&self, v: &Version) -> bool {
        if v.major != self.major {
            return false;
        }
        let Some(m) = self.minor else { return true };
        let Some(p) = self.patch else {
            return if self.major > 0 { v.minor >= m } else { v.minor == m };
        };
        if self.major > 0 {
            if v.minor != m {
                return v.minor > m;
            }
            if v.patch != p {
                return v.patch > p;
            }
        } else if m > 0 {
            if v.minor != m {
                return false;
            }
            if v.patch != p {
                return v.patch > p;
            }
        } else if v.minor != m || v.patch != p {
            return false;
        }
        cmp_pre(&v.pre, &self.pre) != Ordering::Less
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(req: &str, v: &str) -> bool {
        VersionReq::parse(req).unwrap().matches(&Version::parse(v).unwrap())
    }

    #[test]
    fn caret_and_default() {
        assert!(m("1.2.3", "1.9.0") && !m("1.2.3", "2.0.0") && !m("1.2.3", "1.2.2"));
        assert!(m("^0.2.3", "0.2.9") && !m("^0.2.3", "0.3.0"));
        assert!(m("^0.0.3", "0.0.3") && !m("^0.0.3", "0.0.4"));
        assert!(m("0.0", "0.0.7") && !m("0.0", "0.1.0"));
        assert!(m("0", "0.9.0") && !m("0", "1.0.0"));
        assert!(m("1", "1.99.0") && !m("1", "2.0.0"));
    }

    #[test]
    fn other_ops() {
        assert!(m("~1.2.3", "1.2.9") && !m("~1.2.3", "1.3.0"));
        assert!(m("~1", "1.5.0") && !m("~1", "2.0.0"));
        assert!(m("=1.2.3", "1.2.3") && !m("=1.2.3", "1.2.4"));
        assert!(m("=1.2", "1.2.7") && !m("=1.2", "1.3.0"));
        assert!(m(">=1.2, <1.5", "1.4.9") && !m(">=1.2, <1.5", "1.5.0"));
        assert!(m(">1.2", "1.3.0") && !m(">1.2", "1.2.9"));
        assert!(m("<=1.2", "1.2.9") && !m("<=1.2", "1.3.0"));
        assert!(m("1.*", "1.4.0") && !m("1.*", "2.0.0") && m("*", "3.1.4"));
        assert!(m("1.2.*", "1.2.5") && !m("1.2.*", "1.3.0"));
        // (no component is incremented: the largest major still matches)
        let max = format!("{}.0.0", u64::MAX);
        assert!(m(&format!("^{max}"), &max) && m(&format!("~{max}"), &max) && m(&format!("<={}", u64::MAX), &max));
    }

    #[test]
    fn pre_releases() {
        assert!(!m("1", "1.5.0-alpha"));
        assert!(m("^1.5.0-alpha", "1.5.0-beta"));
        assert!(!m("^1.5.0-alpha", "1.6.0-alpha"));
        assert!(m("^1.5.0-alpha", "1.6.0"));
        assert!(!m("<2.0.0", "2.0.0-rc.1"));
        // (a partial upper bound covers its whole line, pre-releases too)
        assert!(!m(">=1.2.0-alpha, <1.2", "1.2.0-alpha"));
        assert!(m(">=1.2.0-alpha, <1.2.0", "1.2.0-alpha"));
        assert!(Version::parse("1.0.0-alpha.2").unwrap() < Version::parse("1.0.0-alpha.10").unwrap());
        assert!(Version::parse("1.0.0-rc.1").unwrap() < Version::parse("1.0.0").unwrap());
        assert_eq!(Version::parse("1.2.2+spec-1.1.0").unwrap().to_string(), "1.2.2");
    }
}
