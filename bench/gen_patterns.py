#!/usr/bin/env python3
"""Generated programs in the shapes real Rust code takes, for benchmarks that reach more of the compiler than gen.py
(straight-line functions) and gen_traits.py (trait-heavy generic code) do.

usage: python3 bench/gen_patterns.py PATTERN N [variant] > p.rs
       python3 bench/gen_patterns.py --list

PATTERN is one of the names below, or `mix` (every pattern, N groups each: one program the shape of an application
crate). N is the number of groups; each group is independent code with its own names, so N scales the program
linearly. `variant` changes one constant in the first group's code (the "edit one function" of the KPIs).

Every program is valid Rust (edition 2021) that zustc and rustc both build; main prints one checksum per group, so
the output can be compared with rustc's.

Patterns (what code of the kind each one stands for does):
- interp: a tokenizer over chars, a recursive-descent parser into a Box'ed enum AST, and an evaluator: compilers,
  interpreters, query engines. Recursion, enums with data, exhaustive match, Box.
- records: lines of text parsed into structs (split, parse, Option), grouped in a HashMap, sorted, formatted: CLI
  tools, log and CSV processing. String handling, collections, closures, format!.
- errors: a custom error enum with Display and From impls, validation through Result and `?`, Option combinators:
  services and request handling. The `?` desugaring, From, trait impls on enums.
- state: state machines over enums that carry data, transitions with match guards, events in a Vec: protocols,
  games, workflow engines. Nested match, tuple patterns, impl blocks.
- plugins: handlers as Box<dyn Trait> in a registry, default methods, a context shared through Rc<RefCell<..>>: GUI
  and application frameworks, middleware. Trait objects, interior mutability, dynamic dispatch.
- borrows: zero-copy parsing with a struct holding &'a str, methods returning slices of the input, &mut iteration:
  parsers, protocol decoders. Lifetimes in structs and signatures, the borrow checker on real shapes.
- macros: macro_rules! that generate structs, impls and lookup tables, with repetition: codegen-heavy crates
  (bitflags-, enum-table-style code). Macro expansion.
- modules: nested modules with pub(crate) items, use paths, constants and statics, impls split across modules:
  the layout of large applications. Name resolution and visibility.
- generics: generic containers and functions instantiated at several types, traits with generic methods, iterator
  adaptors written by hand: library code. Monomorphization, trait method resolution, inference.
"""
import sys


def interp(i, k):
    return f"""
mod interp{i} {{
    #[derive(Debug, Clone, PartialEq)]
    pub enum Tok {{ Num(i64), Ident(String), Op(char), LParen, RParen }}

    pub fn lex(src: &str) -> Vec<Tok> {{
        let cs: Vec<char> = src.chars().collect();
        let mut out = Vec::new();
        let mut p = 0;
        while p < cs.len() {{
            let c = cs[p];
            if c.is_ascii_digit() {{
                let mut n = 0i64;
                while p < cs.len() && cs[p].is_ascii_digit() {{
                    n = n * 10 + cs[p].to_digit(10).unwrap() as i64;
                    p += 1;
                }}
                out.push(Tok::Num(n));
                continue;
            }}
            if c.is_ascii_alphabetic() {{
                let mut s = String::new();
                while p < cs.len() && cs[p].is_ascii_alphanumeric() {{
                    s.push(cs[p]);
                    p += 1;
                }}
                out.push(Tok::Ident(s));
                continue;
            }}
            match c {{
                '(' => out.push(Tok::LParen),
                ')' => out.push(Tok::RParen),
                '+' | '-' | '*' | '/' | '%' => out.push(Tok::Op(c)),
                _ => {{}}
            }}
            p += 1;
        }}
        out
    }}

    #[derive(Debug)]
    pub enum Expr {{
        Num(i64),
        Var(String),
        Bin(char, Box<Expr>, Box<Expr>),
        Neg(Box<Expr>),
        Let(String, Box<Expr>, Box<Expr>),
    }}

    pub struct Parser {{ toks: Vec<Tok>, pos: usize }}

    impl Parser {{
        pub fn new(toks: Vec<Tok>) -> Self {{ Parser {{ toks, pos: 0 }} }}
        fn peek(&self) -> Option<&Tok> {{ self.toks.get(self.pos) }}
        fn next(&mut self) -> Option<Tok> {{
            let t = self.toks.get(self.pos).cloned();
            self.pos += 1;
            t
        }}
        pub fn expr(&mut self) -> Expr {{
            let mut lhs = self.term();
            while let Some(Tok::Op(c)) = self.peek() {{
                let c = *c;
                if c != '+' && c != '-' {{ break; }}
                self.next();
                let rhs = self.term();
                lhs = Expr::Bin(c, Box::new(lhs), Box::new(rhs));
            }}
            lhs
        }}
        fn term(&mut self) -> Expr {{
            let mut lhs = self.atom();
            while let Some(Tok::Op(c)) = self.peek() {{
                let c = *c;
                if c != '*' && c != '/' && c != '%' {{ break; }}
                self.next();
                let rhs = self.atom();
                lhs = Expr::Bin(c, Box::new(lhs), Box::new(rhs));
            }}
            lhs
        }}
        fn atom(&mut self) -> Expr {{
            match self.next() {{
                Some(Tok::Num(n)) => Expr::Num(n),
                Some(Tok::Ident(s)) if s == "let" => {{
                    let name = match self.next() {{ Some(Tok::Ident(n)) => n, _ => String::new() }};
                    let value = self.atom();
                    let body = self.atom();
                    Expr::Let(name, Box::new(value), Box::new(body))
                }}
                Some(Tok::Ident(s)) => Expr::Var(s),
                Some(Tok::Op('-')) => Expr::Neg(Box::new(self.atom())),
                Some(Tok::LParen) => {{
                    let e = self.expr();
                    self.next();
                    e
                }}
                _ => Expr::Num(0),
            }}
        }}
    }}

    pub fn eval(e: &Expr, env: &mut Vec<(String, i64)>) -> i64 {{
        match e {{
            Expr::Num(n) => *n,
            Expr::Var(v) => env.iter().rev().find(|(n, _)| n == v).map(|(_, x)| *x).unwrap_or({k}),
            Expr::Neg(x) => -eval(x, env),
            Expr::Bin(op, a, b) => {{
                let (x, y) = (eval(a, env), eval(b, env));
                match op {{
                    '+' => x.wrapping_add(y),
                    '-' => x.wrapping_sub(y),
                    '*' => x.wrapping_mul(y),
                    '/' => if y == 0 {{ 0 }} else {{ x / y }},
                    _ => if y == 0 {{ 0 }} else {{ x % y }},
                }}
            }}
            Expr::Let(n, v, b) => {{
                let x = eval(v, env);
                env.push((n.clone(), x));
                let r = eval(b, env);
                env.pop();
                r
            }}
        }}
    }}

    pub fn run() -> u64 {{
        let progs = ["1 + 2 * 3", "(4 + {i}) * (7 - 2) % 5", "let x 10 (x * x - 3)", "-(8 / 2) + y", "let a 3 let b (a * a) (b + a * {i})"];
        let mut sum = 0u64;
        for p in progs.iter() {{
            let ast = Parser::new(lex(p)).expr();
            sum = sum.wrapping_mul(31).wrapping_add(eval(&ast, &mut Vec::new()) as u64);
        }}
        sum
    }}
}}
"""


def records(i, k):
    return f"""
mod records{i} {{
    use std::collections::HashMap;

    #[derive(Debug, Clone)]
    pub struct Entry {{ pub host: String, pub status: u16, pub bytes: u64, pub ms: u32 }}

    #[derive(Debug, Default, Clone)]
    pub struct Stats {{ pub count: u64, pub bytes: u64, pub errors: u64, pub slowest: u32 }}

    pub fn parse(line: &str) -> Option<Entry> {{
        let mut it = line.split_whitespace();
        let host = it.next()?.to_string();
        let status = it.next()?.parse::<u16>().ok()?;
        let bytes = it.next()?.parse::<u64>().ok()?;
        let ms = it.next()?.trim_end_matches("ms").parse::<u32>().ok()?;
        Some(Entry {{ host, status, bytes, ms }})
    }}

    pub fn summarize(lines: &[String]) -> Vec<(String, Stats)> {{
        let mut by: HashMap<String, Stats> = HashMap::new();
        for e in lines.iter().filter_map(|l| parse(l)) {{
            let s = by.entry(e.host.clone()).or_default();
            s.count += 1;
            s.bytes += e.bytes;
            if e.status >= 500 {{ s.errors += 1; }}
            s.slowest = s.slowest.max(e.ms);
        }}
        let mut v: Vec<(String, Stats)> = by.into_iter().collect();
        v.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes).then_with(|| a.0.cmp(&b.0)));
        v
    }}

    pub fn run() -> u64 {{
        let hosts = ["api", "web", "db", "cache"];
        let mut lines = Vec::new();
        for n in 0..40u64 {{
            let h = hosts[(n as usize * 7 + {i}) % hosts.len()];
            let status = if n % 9 == 0 {{ 503 }} else {{ 200 }};
            lines.push(format!("{{}}{{}} {{}} {{}} {{}}ms", h, n % 3, status, n * 37 % 1000 + {k}, n * 13 % 250));
        }}
        lines.push(String::from("garbage line"));
        let mut sum = 0u64;
        for (host, s) in summarize(&lines).iter().take(5) {{
            let report = format!("{{}}: {{}} req, {{}} B, {{}} err, {{}} ms", host, s.count, s.bytes, s.errors, s.slowest);
            sum = sum.wrapping_mul(131).wrapping_add(report.len() as u64 + s.bytes);
        }}
        sum
    }}
}}
"""


def errors(i, k):
    return f"""
mod errors{i} {{
    use std::fmt;

    #[derive(Debug, PartialEq)]
    pub enum AppError {{ Missing(&'static str), BadNumber(String), Range {{ field: &'static str, value: i64 }}, Denied }}

    impl fmt::Display for AppError {{
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {{
            match self {{
                AppError::Missing(name) => write!(f, "missing {{}}", name),
                AppError::BadNumber(s) => write!(f, "bad number {{}}", s),
                AppError::Range {{ field, value }} => write!(f, "{{}} out of range: {{}}", field, value),
                AppError::Denied => write!(f, "denied"),
            }}
        }}
    }}

    impl From<std::num::ParseIntError> for AppError {{
        fn from(_: std::num::ParseIntError) -> Self {{ AppError::BadNumber(String::from("?")) }}
    }}

    pub struct Request<'a> {{ pub fields: Vec<(&'a str, &'a str)> }}

    impl<'a> Request<'a> {{
        fn get(&self, name: &'static str) -> Result<&'a str, AppError> {{
            self.fields.iter().find(|(k, _)| *k == name).map(|(_, v)| *v).ok_or(AppError::Missing(name))
        }}
        fn number(&self, name: &'static str, lo: i64, hi: i64) -> Result<i64, AppError> {{
            let v: i64 = self.get(name)?.trim().parse()?;
            if v < lo || v > hi {{
                return Err(AppError::Range {{ field: name, value: v }});
            }}
            Ok(v)
        }}
    }}

    pub struct Order {{ pub qty: i64, pub price: i64, pub user: String }}

    pub fn handle(r: &Request) -> Result<Order, AppError> {{
        let user = r.get("user")?;
        if user == "banned" {{
            return Err(AppError::Denied);
        }}
        let qty = r.number("qty", 1, 100)?;
        let price = r.number("price", 0, 10_000)?;
        Ok(Order {{ qty, price: price + {k}, user: user.to_string() }})
    }}

    pub fn run() -> u64 {{
        let reqs = vec![
            Request {{ fields: vec![("user", "ann"), ("qty", "3"), ("price", "250")] }},
            Request {{ fields: vec![("user", "bob"), ("qty", "x")] }},
            Request {{ fields: vec![("user", "banned"), ("qty", "1"), ("price", "1")] }},
            Request {{ fields: vec![("qty", "1")] }},
            Request {{ fields: vec![("user", "cy"), ("qty", "500"), ("price", "1")] }},
            Request {{ fields: vec![("user", "di"), ("qty", " {i} "), ("price", "99")] }},
        ];
        let mut sum = 0u64;
        for r in reqs.iter() {{
            let line = match handle(r) {{
                Ok(o) => format!("{{}} x{{}} @ {{}}", o.user, o.qty, o.price),
                Err(e) => e.to_string(),
            }};
            sum = sum.wrapping_mul(257).wrapping_add(line.len() as u64);
            sum = sum.wrapping_add(handle(r).map(|o| (o.qty * o.price) as u64).unwrap_or(1));
        }}
        sum
    }}
}}
"""


def state(i, k):
    return f"""
mod state{i} {{
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Event {{ Connect, Data(u32), Ack(u32), Timeout, Close }}

    #[derive(Debug, Clone, PartialEq)]
    pub enum Conn {{
        Idle,
        Handshake {{ tries: u8 }},
        Open {{ sent: u32, acked: u32, buf: Vec<u32> }},
        Closing(u32),
        Closed {{ reason: &'static str }},
    }}

    impl Conn {{
        pub fn step(self, ev: Event) -> Conn {{
            match (self, ev) {{
                (Conn::Idle, Event::Connect) => Conn::Handshake {{ tries: 0 }},
                (Conn::Handshake {{ tries }}, Event::Timeout) if tries < 2 => Conn::Handshake {{ tries: tries + 1 }},
                (Conn::Handshake {{ .. }}, Event::Timeout) => Conn::Closed {{ reason: "handshake" }},
                (Conn::Handshake {{ .. }}, Event::Ack(_)) => Conn::Open {{ sent: 0, acked: 0, buf: Vec::new() }},
                (Conn::Open {{ sent, acked, mut buf }}, Event::Data(n)) => {{
                    buf.push(n);
                    Conn::Open {{ sent: sent + n, acked, buf }}
                }}
                (Conn::Open {{ sent, acked, buf }}, Event::Ack(n)) if acked + n <= sent => {{
                    Conn::Open {{ sent, acked: acked + n, buf }}
                }}
                (Conn::Open {{ sent, acked, .. }}, Event::Close) => Conn::Closing(sent - acked),
                (Conn::Open {{ .. }}, Event::Timeout) => Conn::Closed {{ reason: "timeout" }},
                (Conn::Closing(left), Event::Ack(n)) if n >= left => Conn::Closed {{ reason: "done" }},
                (Conn::Closing(left), Event::Ack(n)) => Conn::Closing(left - n),
                (s, _) => s,
            }}
        }}
        pub fn score(&self) -> u64 {{
            match self {{
                Conn::Idle => 1,
                Conn::Handshake {{ tries }} => 10 + *tries as u64,
                Conn::Open {{ sent, acked, buf }} => 100 + *sent as u64 * 3 + *acked as u64 + buf.len() as u64,
                Conn::Closing(n) => 1000 + *n as u64,
                Conn::Closed {{ reason }} => 5000 + reason.len() as u64 + {k},
            }}
        }}
    }}

    pub fn run() -> u64 {{
        let script = [Event::Connect, Event::Timeout, Event::Ack(0), Event::Data(5), Event::Data({i} + 1),
                      Event::Ack(3), Event::Ack(100), Event::Close, Event::Ack(2), Event::Ack(9)];
        let mut c = Conn::Idle;
        let mut sum = 0u64;
        for ev in script.iter() {{
            c = c.step(*ev);
            sum = sum.wrapping_mul(17).wrapping_add(c.score());
        }}
        let mut d = Conn::Idle;
        for ev in [Event::Connect, Event::Timeout, Event::Timeout, Event::Timeout, Event::Data(1)].iter() {{
            d = d.step(*ev);
        }}
        sum.wrapping_add(d.score())
    }}
}}
"""


def plugins(i, k):
    return f"""
mod plugins{i} {{
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    pub struct Ctx {{ pub log: Vec<String>, pub counter: u64 }}

    pub trait Handler {{
        fn name(&self) -> &str;
        fn handle(&mut self, input: u64, ctx: &Rc<RefCell<Ctx>>) -> Option<u64>;
        fn priority(&self) -> i32 {{ 0 }}
        fn describe(&self) -> String {{ format!("{{}}({{}})", self.name(), self.priority()) }}
    }}

    pub struct Doubler;
    pub struct Limit {{ pub max: u64, pub hits: u32 }}
    pub struct Tally {{ seen: Vec<u64> }}

    impl Handler for Doubler {{
        fn name(&self) -> &str {{ "doubler" }}
        fn handle(&mut self, input: u64, _ctx: &Rc<RefCell<Ctx>>) -> Option<u64> {{ Some(input * 2) }}
    }}
    impl Handler for Limit {{
        fn name(&self) -> &str {{ "limit" }}
        fn priority(&self) -> i32 {{ 10 }}
        fn handle(&mut self, input: u64, ctx: &Rc<RefCell<Ctx>>) -> Option<u64> {{
            if input > self.max {{
                self.hits += 1;
                ctx.borrow_mut().log.push(format!("limit {{}}", input));
                None
            }} else {{
                Some(input)
            }}
        }}
    }}
    impl Handler for Tally {{
        fn name(&self) -> &str {{ "tally" }}
        fn priority(&self) -> i32 {{ -5 }}
        fn handle(&mut self, input: u64, ctx: &Rc<RefCell<Ctx>>) -> Option<u64> {{
            self.seen.push(input);
            ctx.borrow_mut().counter += input;
            Some(input + self.seen.len() as u64)
        }}
    }}

    pub struct Registry {{ handlers: Vec<Box<dyn Handler>>, ctx: Rc<RefCell<Ctx>> }}

    impl Registry {{
        pub fn new() -> Self {{ Registry {{ handlers: Vec::new(), ctx: Rc::new(RefCell::new(Ctx::default())) }} }}
        pub fn add(&mut self, h: Box<dyn Handler>) {{
            self.handlers.push(h);
            self.handlers.sort_by_key(|h| -h.priority());
        }}
        pub fn dispatch(&mut self, input: u64) -> Option<u64> {{
            let mut v = input;
            for h in self.handlers.iter_mut() {{
                v = h.handle(v, &self.ctx)?;
            }}
            Some(v)
        }}
    }}

    pub fn run() -> u64 {{
        let mut r = Registry::new();
        r.add(Box::new(Tally {{ seen: Vec::new() }}));
        r.add(Box::new(Doubler));
        r.add(Box::new(Limit {{ max: 40 + {k}, hits: 0 }}));
        let mut sum = 0u64;
        for x in 0..30u64 {{
            sum = sum.wrapping_mul(7).wrapping_add(r.dispatch(x * 3 + {i}).unwrap_or(99));
        }}
        let names: Vec<String> = r.handlers.iter().map(|h| h.describe()).collect();
        let ctx = r.ctx.borrow();
        sum.wrapping_add(ctx.counter + ctx.log.len() as u64 + names.join(",").len() as u64)
    }}
}}
"""


def borrows(i, k):
    return f"""
mod borrows{i} {{
    pub struct Cursor<'a> {{ src: &'a str, pos: usize }}

    #[derive(Debug, PartialEq)]
    pub enum Item<'a> {{ Key(&'a str), Value(&'a str), Section(&'a str) }}

    impl<'a> Cursor<'a> {{
        pub fn new(src: &'a str) -> Self {{ Cursor {{ src, pos: 0 }} }}
        fn rest(&self) -> &'a str {{ &self.src[self.pos..] }}
        fn take_until(&mut self, stop: char) -> &'a str {{
            let rest = self.rest();
            let n = rest.find(stop).unwrap_or(rest.len());
            self.pos += n;
            &rest[..n]
        }}
        fn skip(&mut self, n: usize) {{ self.pos = (self.pos + n).min(self.src.len()); }}
        pub fn next_item(&mut self) -> Option<Item<'a>> {{
            while self.rest().starts_with('\\n') || self.rest().starts_with(' ') {{
                self.skip(1);
            }}
            if self.rest().is_empty() {{
                return None;
            }}
            if self.rest().starts_with('[') {{
                self.skip(1);
                let name = self.take_until(']');
                self.skip(1);
                return Some(Item::Section(name));
            }}
            let key = self.take_until('=');
            self.skip(1);
            let value = self.take_until('\\n');
            Some(if value.is_empty() {{ Item::Key(key.trim()) }} else {{ Item::Value(value.trim()) }})
        }}
    }}

    pub fn longest<'a>(a: &'a str, b: &'a str) -> &'a str {{ if b.len() > a.len() {{ b }} else {{ a }} }}

    pub fn bump_all(xs: &mut [u64], by: u64) {{
        for x in xs.iter_mut() {{
            *x += by;
        }}
    }}

    pub fn run() -> u64 {{
        let text = String::from("[server]\\nhost = example.org\\nport = 80{i}\\n[client]\\nretries = 3\\ntimeout = 250\\nflag\\n");
        let mut c = Cursor::new(&text);
        let mut lens: Vec<u64> = Vec::new();
        let mut best = "";
        while let Some(item) = c.next_item() {{
            let s = match item {{ Item::Key(s) | Item::Value(s) | Item::Section(s) => s }};
            best = longest(best, s);
            lens.push(s.len() as u64);
        }}
        bump_all(&mut lens, {k} + 1);
        let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        lens.iter().fold(best.len() as u64, |a, b| a.wrapping_mul(13).wrapping_add(*b)) + words.len() as u64
    }}
}}
"""


def macros(i, k):
    return f"""
mod macros{i} {{
    macro_rules! flags {{
        ($name:ident {{ $($flag:ident = $bit:expr),* $(,)? }}) => {{
            #[derive(Debug, Clone, Copy, PartialEq, Default)]
            pub struct $name(u32);
            #[allow(non_upper_case_globals, dead_code)]
            impl $name {{
                $(pub const $flag: $name = $name(1 << $bit);)*
                pub fn contains(&self, o: $name) -> bool {{ self.0 & o.0 == o.0 }}
                pub fn insert(&mut self, o: $name) {{ self.0 |= o.0; }}
                pub fn names(&self) -> Vec<&'static str> {{
                    let mut v = Vec::new();
                    $(if self.contains($name::$flag) {{ v.push(stringify!($flag)); }})*
                    v
                }}
            }}
        }};
    }}

    macro_rules! table {{
        ($fname:ident : $($key:literal => $val:expr),* $(,)?) => {{
            pub fn $fname(k: &str) -> Option<u64> {{
                match k {{
                    $($key => Some($val),)*
                    _ => None,
                }}
            }}
        }};
    }}

    macro_rules! sum_all {{
        () => {{ 0u64 }};
        ($x:expr $(, $rest:expr)*) => {{ $x + sum_all!($($rest),*) }};
    }}

    flags!(Perm {{ Read = 0, Write = 1, Exec = 2, Admin = 3 }});
    table!(weight: "red" => 3, "green" => 5 + {k}, "blue" => 7, "black" => 11, "white" => 13);

    pub fn run() -> u64 {{
        let mut p = Perm::default();
        p.insert(Perm::Read);
        if {i} % 2 == 0 {{ p.insert(Perm::Exec); }}
        let names = p.names().join("|");
        let colors = ["red", "blue", "pink", "white", "green"];
        let w: u64 = colors.iter().filter_map(|c| weight(c)).sum();
        sum_all!(w, names.len() as u64, p.0 as u64, {i})
    }}
}}
"""


def modules(i, k):
    return f"""
mod app{i} {{
    pub mod config {{
        pub const DEFAULT_PORT: u16 = 8080;
        pub static NAMES: [&str; 3] = ["alpha", "beta", "gamma"];
        #[derive(Debug, Clone)]
        pub struct Config {{ pub port: u16, pub workers: usize, pub name: String }}
        impl Config {{
            pub fn new() -> Self {{ Config {{ port: DEFAULT_PORT, workers: 4, name: String::from(NAMES[0]) }} }}
        }}
    }}

    pub mod model {{
        pub mod user {{
            #[derive(Debug, Clone, PartialEq)]
            pub struct User {{ pub id: u32, pub(crate) name: String, pub(super) score: u64 }}
            pub fn make(id: u32, name: &str) -> User {{ User {{ id, name: name.to_string(), score: id as u64 * 3 }} }}
        }}
        pub mod team {{
            use super::user::User;
            pub struct Team {{ pub members: Vec<User> }}
            impl Team {{
                pub fn total(&self) -> u64 {{ self.members.iter().map(|u| u.score + u.name.len() as u64).sum() }}
            }}
        }}
        pub use self::team::Team;
        pub use self::user::{{make, User}};
    }}

    pub mod service {{
        use super::config::Config;
        // (`use super::model;` and then `model::make`: zustc cannot resolve a module brought in by `use` when another
        // module of that name exists, docs/reports/2026-11-20-03-bench-patterns.md; the item is imported instead)
        use super::model::{{make, Team}};
        use crate::app{i}::util::checksum;

        pub(crate) fn build(cfg: &Config) -> Team {{
            let members = (0..cfg.workers as u32).map(|n| make(n, super::config::NAMES[n as usize % 3])).collect();
            Team {{ members }}
        }}
        pub fn report(cfg: &Config) -> u64 {{ checksum(build(cfg).total(), cfg.port as u64) }}
    }}

    mod util {{
        pub fn checksum(a: u64, b: u64) -> u64 {{ a.wrapping_mul(1_000_003) ^ (b + {k}) }}
    }}

    impl config::Config {{
        pub fn scaled(mut self, n: usize) -> Self {{
            self.workers *= n;
            self
        }}
    }}

    pub fn run() -> u64 {{
        let cfg = config::Config::new().scaled({i} % 3 + 1);
        let mut name = cfg.name.clone();
        name.push_str("-x");
        service::report(&cfg) + name.len() as u64 + model::make(7, "z").id as u64
    }}
}}
"""


def generics(i, k):
    # (the trait and its method are named per group: zustc does not tell apart two modules' traits of one name that are
    # implemented for the same type, and names a closure in an impl's method after the type and the method only; and
    # `.map(Tr::m)` (a trait's method as a value) is not supported, so it is a closure. docs/reports/2026-11-20-03-bench-patterns.md)
    return f"""
mod generics{i} {{
    use std::fmt::Debug;

    #[derive(Debug, Clone)]
    pub struct Stack<T> {{ items: Vec<T> }}

    impl<T: Clone + Debug> Stack<T> {{
        pub fn new() -> Self {{ Stack {{ items: Vec::new() }} }}
        pub fn push(&mut self, t: T) -> &mut Self {{
            self.items.push(t);
            self
        }}
        pub fn pop(&mut self) -> Option<T> {{ self.items.pop() }}
        pub fn peek(&self) -> Option<&T> {{ self.items.last() }}
        pub fn map<U, F: Fn(&T) -> U>(&self, f: F) -> Stack<U> {{ Stack {{ items: self.items.iter().map(f).collect() }} }}
        pub fn len(&self) -> usize {{ self.items.len() }}
    }}

    pub trait Measure{i} {{
        fn measure{i}(&self) -> u64;
    }}
    impl Measure{i} for u32 {{ fn measure{i}(&self) -> u64 {{ *self as u64 }} }}
    impl Measure{i} for String {{ fn measure{i}(&self) -> u64 {{ self.len() as u64 }} }}
    impl<A: Measure{i}, B: Measure{i}> Measure{i} for (A, B) {{ fn measure{i}(&self) -> u64 {{ self.0.measure{i}() * 2 + self.1.measure{i}() }} }}
    impl<T: Measure{i}> Measure{i} for Vec<T> {{ fn measure{i}(&self) -> u64 {{ self.iter().map(|t| t.measure{i}()).sum() }} }}
    impl<T: Measure{i}> Measure{i} for Option<T> {{ fn measure{i}(&self) -> u64 {{ self.as_ref().map_or(1, |t| t.measure{i}()) }} }}

    pub fn total<T: Measure{i}>(xs: &[T]) -> u64 {{ xs.iter().map(|x| x.measure{i}()).sum::<u64>() + {k} }}

    pub fn pairwise<A, B, I, J>(a: I, b: J) -> Vec<(A, B)>
    where
        I: IntoIterator<Item = A>,
        J: IntoIterator<Item = B>,
    {{
        a.into_iter().zip(b).collect()
    }}

    pub struct Every<I> {{ inner: I, n: usize }}
    impl<I: Iterator> Iterator for Every<I> {{
        type Item = I::Item;
        fn next(&mut self) -> Option<I::Item> {{
            let x = self.inner.next()?;
            for _ in 1..self.n {{
                self.inner.next();
            }}
            Some(x)
        }}
    }}
    pub fn every<I: Iterator>(inner: I, n: usize) -> Every<I> {{ Every {{ inner, n }} }}

    pub fn run() -> u64 {{
        let mut s: Stack<u32> = Stack::new();
        s.push(1).push(2).push(3 + {i});
        let doubled = s.map(|x| x * 2);
        let words = s.map(|x| format!("w{{}}", x));
        let pairs = pairwise(vec![1u32, 2, 3], vec![String::from("a"), String::from("bb"), String::from("ccc")]);
        let opts: Vec<Option<u32>> = vec![Some(4), None, Some(6)];
        let picked: Vec<u32> = every(0..20u32, 3).collect();
        let mut sum = total(&doubled.items) + total(&words.items) + total(&pairs) + total(&opts) + total(&picked);
        sum += s.pop().map_or(0, |x| x as u64) + s.peek().copied().unwrap_or(0) as u64 + s.len() as u64;
        sum
    }}
}}
"""


PATTERNS = {"interp": interp, "records": records, "errors": errors, "state": state, "plugins": plugins,
            "borrows": borrows, "macros": macros, "modules": modules, "generics": generics}
# (each pattern's module name: interp0, records0, .., modules use app0)
MODNAME = {"modules": "app"}


def program(pattern, n, variant=0):
    names = list(PATTERNS) if pattern == "mix" else [pattern]
    out = ["// generated by bench/gen_patterns.py", "#![allow(dead_code, unused_variables, unused_mut, unused_imports)]"]
    calls = []
    for name in names:
        for i in range(n):
            out.append(PATTERNS[name](i, variant if i == 0 and name == names[0] else 0))
            calls.append(f"{MODNAME.get(name, name)}{i}::run()")
    out.append("fn main() {")
    for c in calls:
        out.append(f'    println!("{{}}", {c});')
    out.append("}")
    return "\n".join(out) + "\n"


def main():
    if sys.argv[1:] == ["--list"]:
        print("\n".join(PATTERNS))
        return
    if len(sys.argv) < 3 or (sys.argv[1] not in PATTERNS and sys.argv[1] != "mix"):
        sys.exit(f"usage: gen_patterns.py ({'|'.join(PATTERNS)}|mix) N [variant]")
    sys.stdout.write(program(sys.argv[1], int(sys.argv[2]), int(sys.argv[3]) if len(sys.argv) > 3 else 0))


if __name__ == "__main__":
    main()
