//! TOML (v1.0) as Cargo.toml, Cargo.lock and .cargo/config.toml use it: tables, arrays of tables, dotted and quoted
//! keys, inline tables, arrays, the four kinds of strings, integers, floats, booleans. A date or time is kept as its
//! text (`Value::Datetime`).

use std::collections::BTreeMap;

pub type Table = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Datetime(String),
    Array(Vec<Value>),
    Table(Table),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    /// The strings of an array (other elements are skipped)
    pub fn str_list(&self) -> Vec<String> {
        self.as_array().unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
    }
}

/// Look up a dotted path of keys (`get(t, &["package", "name"])`)
pub fn get<'a>(t: &'a Table, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let v = t.get(*first)?;
    if rest.is_empty() { Some(v) } else { get(v.as_table()?, rest) }
}

pub fn get_str<'a>(t: &'a Table, path: &[&str]) -> Option<&'a str> {
    get(t, path).and_then(Value::as_str)
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    line: usize,
}

type R<T> = Result<T, String>;

pub fn parse(text: &str) -> R<Table> {
    let mut p = P { s: text.as_bytes(), i: 0, line: 1 };
    let mut root = Table::new();
    // (the table that `key = value` lines go into: a path from the root; an array of tables means its last element)
    let mut cur: Vec<String> = vec![];
    // (tables defined by a header, and the arrays of tables: a second `[a]` is an error, `[[a]]` appends)
    let mut defined: Vec<Vec<String>> = vec![];
    loop {
        p.skip_ws_comments_newlines();
        if p.i >= p.s.len() {
            break;
        }
        if p.peek() == Some(b'[') {
            let array = p.s.get(p.i + 1) == Some(&b'[');
            p.i += if array { 2 } else { 1 };
            p.skip_ws();
            let key = p.key()?;
            p.skip_ws();
            if array {
                p.expect(b']')?;
                p.expect(b']')?;
                // (a new element: the tables defined under the previous one may be defined again in it)
                defined.retain(|d| !d.starts_with(&key));
                let parent = walk(&mut root, &key[..key.len() - 1], p.line)?;
                let last = key.last().unwrap().clone();
                match parent.entry(last).or_insert_with(|| Value::Array(vec![])) {
                    Value::Array(a) => a.push(Value::Table(Table::new())),
                    _ => return Err(p.err("an array of tables over a value of another kind")),
                }
            } else {
                p.expect(b']')?;
                if defined.contains(&key) {
                    return Err(p.err(&format!("table `{}` defined twice", key.join("."))));
                }
                defined.push(key.clone());
                walk(&mut root, &key, p.line)?;
            }
            cur = key;
            p.end_of_line()?;
            continue;
        }
        let key = p.key()?;
        p.skip_ws();
        p.expect(b'=')?;
        p.skip_ws();
        let v = p.value()?;
        let line = p.line;
        let t = walk(&mut root, &cur, line)?;
        insert(t, &key, v, line)?;
        p.end_of_line()?;
    }
    Ok(root)
}

/// The table at `path` (made if missing). An array of tables on the way means its last element
fn walk<'t>(mut t: &'t mut Table, path: &[String], line: usize) -> R<&'t mut Table> {
    for k in path {
        let v = t.entry(k.clone()).or_insert_with(|| Value::Table(Table::new()));
        t = match v {
            Value::Table(x) => x,
            Value::Array(a) => match a.last_mut() {
                Some(Value::Table(x)) => x,
                _ => return Err(format!("line {line}: `{k}` is not a table")),
            },
            _ => return Err(format!("line {line}: `{k}` is not a table")),
        };
    }
    Ok(t)
}

fn insert(t: &mut Table, key: &[String], v: Value, line: usize) -> R<()> {
    let (last, path) = key.split_last().unwrap();
    let t = walk(t, path, line)?;
    if t.contains_key(last) {
        return Err(format!("line {line}: key `{}` defined twice", key.join(".")));
    }
    t.insert(last.clone(), v);
    Ok(())
}

impl P<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn err(&self, m: &str) -> String {
        format!("line {}: {m}", self.line)
    }
    fn expect(&mut self, c: u8) -> R<()> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected `{}`", c as char)))
        }
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
    }
    fn skip_comment(&mut self) {
        if self.peek() == Some(b'#') {
            while !matches!(self.peek(), None | Some(b'\n')) {
                self.i += 1;
            }
        }
    }
    fn skip_ws_comments_newlines(&mut self) {
        loop {
            self.skip_ws();
            self.skip_comment();
            match self.peek() {
                Some(b'\n') => {
                    self.i += 1;
                    self.line += 1;
                }
                Some(b'\r') => self.i += 1,
                _ => break,
            }
        }
    }
    fn end_of_line(&mut self) -> R<()> {
        self.skip_ws();
        self.skip_comment();
        match self.peek() {
            None | Some(b'\n') | Some(b'\r') => Ok(()),
            Some(c) => Err(self.err(&format!("unexpected `{}` after a value", c as char))),
        }
    }
    /// `a.b."c d".'e'`
    fn key(&mut self) -> R<Vec<String>> {
        let mut out = vec![];
        loop {
            self.skip_ws();
            let part = match self.peek() {
                Some(b'"') => self.basic_string()?,
                Some(b'\'') => self.literal_string()?,
                _ => {
                    let a = self.i;
                    while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
                        self.i += 1;
                    }
                    if a == self.i {
                        return Err(self.err("expected a key"));
                    }
                    String::from_utf8_lossy(&self.s[a..self.i]).into_owned()
                }
            };
            out.push(part);
            self.skip_ws();
            if self.peek() == Some(b'.') {
                self.i += 1;
            } else {
                return Ok(out);
            }
        }
    }
    fn value(&mut self) -> R<Value> {
        match self.peek() {
            Some(b'"') => {
                if self.s[self.i..].starts_with(b"\"\"\"") {
                    self.multiline(b'"').map(Value::String)
                } else {
                    self.basic_string().map(Value::String)
                }
            }
            Some(b'\'') => {
                if self.s[self.i..].starts_with(b"'''") {
                    self.multiline(b'\'').map(Value::String)
                } else {
                    self.literal_string().map(Value::String)
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = vec![];
                loop {
                    self.skip_ws_comments_newlines();
                    if self.peek() == Some(b']') {
                        self.i += 1;
                        return Ok(Value::Array(a));
                    }
                    a.push(self.value()?);
                    self.skip_ws_comments_newlines();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {}
                        _ => return Err(self.err("expected `,` or `]` in an array")),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut t = Table::new();
                self.skip_ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Value::Table(t));
                }
                loop {
                    self.skip_ws();
                    let k = self.key()?;
                    self.skip_ws();
                    self.expect(b'=')?;
                    self.skip_ws();
                    let v = self.value()?;
                    insert(&mut t, &k, v, self.line)?;
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Table(t));
                        }
                        _ => return Err(self.err("expected `,` or `}` in an inline table")),
                    }
                }
            }
            _ => self.scalar(),
        }
    }
    /// A bare value: a boolean, a number, or a date / time
    fn scalar(&mut self) -> R<Value> {
        let a = self.i;
        // (a date-time may hold one space: `1979-05-27 07:32:00`)
        while let Some(c) = self.peek() {
            let date_space = c == b' ' && self.i - a == 10 && self.s[a..self.i].iter().filter(|&&x| x == b'-').count() == 2
                && self.s.get(self.i + 1).is_some_and(u8::is_ascii_digit);
            if matches!(c, b',' | b']' | b'}' | b'\n' | b'\r' | b'#' | b'\t') || (c == b' ' && !date_space) {
                break;
            }
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[a..self.i]).map_err(|_| self.err("bad UTF-8"))?;
        match t {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            "" => return Err(self.err("expected a value")),
            _ => {}
        }
        let clean: String = t.chars().filter(|&c| c != '_').collect();
        let (neg, body) = match clean.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, clean.strip_prefix('+').unwrap_or(&clean)),
        };
        let radix = [("0x", 16), ("0o", 8), ("0b", 2)].iter().find(|(p, _)| body.starts_with(p));
        if let Some((p, r)) = radix {
            let v = i64::from_str_radix(&body[p.len()..], *r).map_err(|_| self.err(&format!("bad integer `{t}`")))?;
            return Ok(Value::Integer(if neg { -v } else { v }));
        }
        if let Ok(v) = clean.parse::<i64>() {
            return Ok(Value::Integer(v));
        }
        if matches!(body, "inf" | "nan") || clean.parse::<f64>().is_ok() {
            let v = match body {
                "inf" => f64::INFINITY,
                "nan" => f64::NAN,
                _ => body.parse::<f64>().unwrap(),
            };
            return Ok(Value::Float(if neg { -v } else { v }));
        }
        if t.as_bytes().first().is_some_and(u8::is_ascii_digit) && (t.contains('-') || t.contains(':')) {
            return Ok(Value::Datetime(t.to_string()));
        }
        Err(self.err(&format!("unexpected value `{t}`")))
    }
    fn basic_string(&mut self) -> R<String> {
        self.expect(b'"')?;
        let mut out: Vec<u8> = vec![];
        loop {
            match self.peek() {
                None | Some(b'\n') => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return String::from_utf8(out).map_err(|_| self.err("bad UTF-8"));
                }
                Some(b'\\') => {
                    self.i += 1;
                    self.escape(&mut out)?;
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }
    fn escape(&mut self, out: &mut Vec<u8>) -> R<()> {
        let c = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
        self.i += 1;
        let ch = match c {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'e' => '\u{1b}',
            b'"' => '"',
            b'\\' => '\\',
            b'u' | b'U' => {
                let n = if c == b'u' { 4 } else { 8 };
                let h = std::str::from_utf8(self.s.get(self.i..self.i + n).unwrap_or(b"")).unwrap_or("");
                self.i += n;
                u32::from_str_radix(h, 16).ok().and_then(char::from_u32).ok_or_else(|| self.err("bad unicode escape"))?
            }
            _ => return Err(self.err(&format!("bad escape `\\{}`", c as char))),
        };
        let mut b = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
        Ok(())
    }
    fn literal_string(&mut self) -> R<String> {
        self.expect(b'\'')?;
        let a = self.i;
        while !matches!(self.peek(), None | Some(b'\'') | Some(b'\n')) {
            self.i += 1;
        }
        if self.peek() != Some(b'\'') {
            return Err(self.err("unterminated string"));
        }
        let t = String::from_utf8_lossy(&self.s[a..self.i]).into_owned();
        self.i += 1;
        Ok(t)
    }
    /// `"""..."""` (escapes, and a `\` at the end of a line joins it with the next) or `'''...'''`
    fn multiline(&mut self, q: u8) -> R<String> {
        self.i += 3;
        // (a newline right after the opening quotes is not part of the string)
        if self.s[self.i..].starts_with(b"\r\n") {
            self.i += 2;
            self.line += 1;
        } else if self.peek() == Some(b'\n') {
            self.i += 1;
            self.line += 1;
        }
        let mut out: Vec<u8> = vec![];
        loop {
            let rest = &self.s[self.i..];
            if rest.is_empty() {
                return Err(self.err("unterminated multi-line string"));
            }
            if rest.starts_with(&[q, q, q]) {
                // (up to two more quotes belong to the string: `""""` ends with one `"` inside)
                let mut n = 3;
                while self.s.get(self.i + n) == Some(&q) && n < 5 {
                    n += 1;
                }
                out.extend(std::iter::repeat_n(q, n - 3));
                self.i += n;
                return String::from_utf8(out).map_err(|_| self.err("bad UTF-8"));
            }
            let c = rest[0];
            if c == b'\n' {
                self.line += 1;
            }
            if q == b'"' && c == b'\\' {
                self.i += 1;
                // (line-ending backslash: drop the newline and the whitespace that follows)
                let mut k = self.i;
                while matches!(self.s.get(k), Some(b' ' | b'\t')) {
                    k += 1;
                }
                if matches!(self.s.get(k), Some(b'\n' | b'\r')) {
                    self.i = k;
                    while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                        if self.peek() == Some(b'\n') {
                            self.line += 1;
                        }
                        self.i += 1;
                    }
                    continue;
                }
                self.escape(&mut out)?;
                continue;
            }
            out.push(c);
            self.i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }

    #[test]
    fn tables_keys_and_values() {
        let t = parse(
            r#"
# comment
[package]
name = "foo-bar" # trailing
version = "0.1.0"
edition = '2021'
authors = [
  "a <a@b>",   # inside an array
  "c",
]

[dependencies]
serde = { version = "1", features = ["derive"], default-features = false }
local.path = "../local"
"quoted key" = 1_000

[target.'cfg(unix)'.dependencies.libc]
version = "0.2"

[[bin]]
name = "a"
[[bin]]
name = "b"
path = "src/b.rs"
[bin.extra]
x = true
"#,
        )
        .unwrap();
        assert_eq!(get(&t, &["package", "name"]), Some(&s("foo-bar")));
        assert_eq!(get(&t, &["package", "edition"]), Some(&s("2021")));
        assert_eq!(get(&t, &["package", "authors"]).unwrap().str_list(), vec!["a <a@b>", "c"]);
        assert_eq!(get(&t, &["dependencies", "serde", "default-features"]), Some(&Value::Bool(false)));
        assert_eq!(get(&t, &["dependencies", "serde", "features"]).unwrap().str_list(), vec!["derive"]);
        assert_eq!(get(&t, &["dependencies", "local", "path"]), Some(&s("../local")));
        assert_eq!(get(&t, &["dependencies", "quoted key"]), Some(&Value::Integer(1000)));
        assert_eq!(get(&t, &["target", "cfg(unix)", "dependencies", "libc", "version"]), Some(&s("0.2")));
        let bins = get(&t, &["bin"]).unwrap().as_array().unwrap();
        assert_eq!(bins.len(), 2);
        assert_eq!(get(bins[1].as_table().unwrap(), &["extra", "x"]), Some(&Value::Bool(true)));
    }

    #[test]
    fn strings() {
        let t = parse("a = \"x\\ty\\u00e9\\\"\"\nb = '''\nraw\\n'''\nc = \"\"\"\nfirst \\\n   second\"\"\"\nd = \"\"\"q\"\"\"\"\n").unwrap();
        assert_eq!(t["a"], s("x\ty\u{e9}\""));
        assert_eq!(t["b"], s("raw\\n"));
        assert_eq!(t["c"], s("first second"));
        assert_eq!(t["d"], s("q\""));
    }

    #[test]
    fn numbers_and_dates() {
        let t = parse("a = -0x10\nb = 3.5e2\nc = +7\nd = 1979-05-27T07:32:00Z\ne = 1979-05-27 07:32:00\nf = inf\n").unwrap();
        assert_eq!(t["a"], Value::Integer(-16));
        assert_eq!(t["b"], Value::Float(350.0));
        assert_eq!(t["c"], Value::Integer(7));
        assert_eq!(t["d"], Value::Datetime("1979-05-27T07:32:00Z".into()));
        assert_eq!(t["e"], Value::Datetime("1979-05-27 07:32:00".into()));
        assert_eq!(t["f"], Value::Float(f64::INFINITY));
    }

    #[test]
    fn tables_under_array_elements() {
        let t = parse("[[items]]\nname = \"a\"\n[items.detail]\nx = 1\n[[items]]\nname = \"b\"\n[items.detail]\nx = 2\n").unwrap();
        let items = t["items"].as_array().unwrap();
        assert_eq!(get(items[1].as_table().unwrap(), &["detail", "x"]), Some(&Value::Integer(2)));
        assert!(parse("[[items]]\n[items.detail]\n[items.detail]\n").is_err());
    }

    #[test]
    fn errors() {
        assert!(parse("[a]\nx = 1\n[a]\n").is_err());
        assert!(parse("x = 1\nx = 2\n").is_err());
        assert!(parse("x = \"open\n").is_err());
        assert!(parse("x = 1 2\n").is_err());
    }
}
