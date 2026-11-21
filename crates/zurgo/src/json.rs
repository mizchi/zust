//! A JSON reader for cargo's registry index (one JSON object per version). Values are read into `toml::Value`, so the
//! same lookups work: objects are tables, `null` leaves the key out (and is skipped in arrays).

use crate::toml::{Table, Value};

pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = P { s: text.as_bytes(), i: 0 };
    p.ws();
    let v = p.value()?.unwrap_or(Value::Table(Table::new()));
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("JSON: trailing data at byte {}", p.i));
    }
    Ok(v)
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }
    fn err<T>(&self, m: &str) -> Result<T, String> {
        Err(format!("JSON: {m} at byte {}", self.i))
    }
    fn lit(&mut self, w: &str) -> bool {
        if self.s[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            true
        } else {
            false
        }
    }
    /// A value; `None` for `null`
    fn value(&mut self) -> Result<Option<Value>, String> {
        match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut t = Table::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Some(Value::Table(t)));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if !self.lit(":") {
                        return self.err("expected `:`");
                    }
                    self.ws();
                    if let Some(v) = self.value()? {
                        t.insert(k, v);
                    }
                    self.ws();
                    if self.lit(",") {
                        continue;
                    }
                    if self.lit("}") {
                        return Ok(Some(Value::Table(t)));
                    }
                    return self.err("expected `,` or `}`");
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = vec![];
                self.ws();
                if self.lit("]") {
                    return Ok(Some(Value::Array(a)));
                }
                loop {
                    self.ws();
                    if let Some(v) = self.value()? {
                        a.push(v);
                    }
                    self.ws();
                    if self.lit(",") {
                        continue;
                    }
                    if self.lit("]") {
                        return Ok(Some(Value::Array(a)));
                    }
                    return self.err("expected `,` or `]`");
                }
            }
            Some(b'"') => Ok(Some(Value::String(self.string()?))),
            Some(b't') if self.lit("true") => Ok(Some(Value::Bool(true))),
            Some(b'f') if self.lit("false") => Ok(Some(Value::Bool(false))),
            Some(b'n') if self.lit("null") => Ok(None),
            Some(c) if c.is_ascii_digit() || *c == b'-' => {
                let a = self.i;
                while matches!(self.s.get(self.i), Some(c) if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E')) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[a..self.i]).unwrap_or("");
                match t.parse::<i64>() {
                    Ok(n) => Ok(Some(Value::Integer(n))),
                    Err(_) => t.parse::<f64>().map(|f| Some(Value::Float(f))).or_else(|_| self.err("bad number")),
                }
            }
            _ => self.err("expected a value"),
        }
    }
    fn string(&mut self) -> Result<String, String> {
        if !self.lit("\"") {
            return self.err("expected a string");
        }
        let mut out: Vec<u8> = vec![];
        loop {
            let Some(&c) = self.s.get(self.i) else { return self.err("unterminated string") };
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).or_else(|_| self.err("bad UTF-8")),
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else { return self.err("unterminated escape") };
                    self.i += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'/' => '/',
                        b'\\' => '\\',
                        b'"' => '"',
                        b'u' => {
                            let hex = |p: &mut Self| -> Result<u32, String> {
                                let h = std::str::from_utf8(p.s.get(p.i..p.i + 4).unwrap_or(b"")).unwrap_or("");
                                p.i += 4;
                                u32::from_str_radix(h, 16).or_else(|_| p.err("bad \\u escape"))
                            };
                            let mut u = hex(self)?;
                            // (a surrogate pair: the second half must be a low surrogate)
                            if (0xd800..0xdc00).contains(&u) {
                                if !self.lit("\\u") {
                                    return self.err("a high surrogate without its low half");
                                }
                                let lo = hex(self)?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return self.err("a high surrogate followed by no low surrogate");
                                }
                                u = 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00);
                            }
                            match char::from_u32(u) {
                                Some(c) => c,
                                None => return self.err("a lone surrogate"),
                            }
                        }
                        _ => return self.err("bad escape"),
                    };
                    let mut b = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                }
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        let v = parse(r#"{"name":"aé😀","n":-12,"f":1.5,"t":true,"z":null,"arr":[1,null,"x"],"o":{}}"#).unwrap();
        let t = v.as_table().unwrap();
        assert_eq!(t["name"], Value::String("a\u{e9}\u{1f600}".into()));
        assert_eq!(t["n"], Value::Integer(-12));
        assert_eq!(t["f"], Value::Float(1.5));
        assert_eq!(t["t"], Value::Bool(true));
        assert!(!t.contains_key("z"));
        assert_eq!(t["arr"], Value::Array(vec![Value::Integer(1), Value::String("x".into())]));
        assert!(parse("{\"a\":1,}").is_err());
        assert!(parse(r#""\ud83dabc""#).is_err() && parse(r#""\ud83d\u0041""#).is_err() && parse(r#""\ude00""#).is_err());
        assert!(parse("[1] 2").is_err());
    }
}
