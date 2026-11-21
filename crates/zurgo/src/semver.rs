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

impl Comparator {
    fn matches(&self, v: &Version) -> bool {
        let lo = Version { major: self.major, minor: self.minor.unwrap_or(0), patch: self.patch.unwrap_or(0), pre: self.pre.clone() };
        let ver = |a: u64, b: u64, c: u64| Version { major: a, minor: b, patch: c, pre: vec![] };
        // (the smallest version above the range of a partial version: `1.2` → 1.3.0, `1` → 2.0.0)
        let after = || match (self.minor, self.patch) {
            (None, _) => ver(self.major + 1, 0, 0),
            (Some(m), None) => ver(self.major, m + 1, 0),
            (Some(m), Some(p)) => ver(self.major, m, p + 1),
        };
        let in_range = |hi: Version| *v >= lo && cmp_core(v, &hi) == Ordering::Less;
        match self.op {
            Op::Exact => match (self.minor, self.patch) {
                (Some(_), Some(_)) => *v == lo,
                _ => in_range(after()),
            },
            Op::Greater => match (self.minor, self.patch) {
                (Some(_), Some(_)) => *v > lo,
                _ => *v >= after(),
            },
            Op::GreaterEq => *v >= lo,
            Op::Less => cmp_core(v, &lo) == Ordering::Less || (*v < lo && !v.pre.is_empty()),
            Op::LessEq => match (self.minor, self.patch) {
                (Some(_), Some(_)) => *v <= lo,
                _ => cmp_core(v, &after()) == Ordering::Less,
            },
            Op::Tilde => match self.minor {
                None => in_range(ver(self.major + 1, 0, 0)),
                Some(m) => in_range(ver(self.major, m + 1, 0)),
            },
            Op::Wildcard => in_range(after()),
            Op::Caret => {
                let hi = match (self.major, self.minor, self.patch) {
                    (0, None, _) => ver(1, 0, 0),
                    (0, Some(0), None) => ver(0, 1, 0),
                    (0, Some(0), Some(p)) => ver(0, 0, p + 1),
                    (0, Some(m), _) => ver(0, m + 1, 0),
                    (a, _, _) => ver(a + 1, 0, 0),
                };
                in_range(hi)
            }
        }
    }
}

/// Compare as if `b` were a release: an upper bound `<2.0.0` excludes `2.0.0-alpha` too
fn cmp_core(a: &Version, b: &Version) -> Ordering {
    let o = (a.major, a.minor, a.patch).cmp(&(b.major, b.minor, b.patch));
    if o == Ordering::Equal && b.pre.is_empty() { Ordering::Equal } else { o.then_with(|| cmp_pre(&a.pre, &b.pre)) }
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
    }

    #[test]
    fn pre_releases() {
        assert!(!m("1", "1.5.0-alpha"));
        assert!(m("^1.5.0-alpha", "1.5.0-beta"));
        assert!(!m("^1.5.0-alpha", "1.6.0-alpha"));
        assert!(m("^1.5.0-alpha", "1.6.0"));
        assert!(!m("<2.0.0", "2.0.0-rc.1"));
        assert!(Version::parse("1.0.0-alpha.2").unwrap() < Version::parse("1.0.0-alpha.10").unwrap());
        assert!(Version::parse("1.0.0-rc.1").unwrap() < Version::parse("1.0.0").unwrap());
        assert_eq!(Version::parse("1.2.2+spec-1.1.0").unwrap().to_string(), "1.2.2");
    }
}
