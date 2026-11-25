//! The session's cache (on disk, `<output>.cache`), without external crates (zstd):
//! - key: a 128-bit hash (the low 64 bits are written to the file)
//! - value: an item's summary, or a fn's machine code + symbolic relocations (acg::Blob; callee instances are written as (original fn name, type arguments))
//! - saving writes only the entries this build used (old ones disappear naturally)
//!
//! The serialization of Blob is also used to receive compilation results from forked child processes (session's parallelization).

use std::sync::Mutex;

use crate::acg::{Blob, BlobReloc, Target};
use crate::fxhash::FxMap;
use crate::program::{CloFact, CloRg, Insts, Program};

pub use crate::key::{Key, KeyHasher};

const MAGIC: &[u8; 8] = b"CRCD0002";

unsafe extern "C" {
    #[link_name = "rename"]
    fn c_rename(old: *const u8, new: *const u8) -> i32;
    #[link_name = "getpid"]
    fn c_getpid() -> i32;
}

/// The cache file: MAGIC, number of entries (varint), (low 64 bits of the key, length (varint), contents) *
pub struct BuildCache {
    data: Vec<u8>,
    index: FxMap<u64, (usize, usize)>,
}

pub struct Rd<'a> {
    pub d: &'a [u8],
    pub pos: usize,
}

impl Rd<'_> {
    pub fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(b[0] as u16 | (b[1] as u16) << 8)
    }
    pub fn u64(&mut self) -> Option<u64> {
        let b = self.take(8)?;
        let mut v = 0u64;
        for i in 0..8 {
            v |= (b[i] as u64) << (8 * i);
        }
        Some(v)
    }
    pub fn uv(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut sh = 0;
        loop {
            let b = self.u8()?;
            v |= ((b & 0x7F) as u64) << sh;
            if b < 0x80 {
                return Some(v);
            }
            sh += 7;
            if sh > 63 {
                return None;
            }
        }
    }
    pub fn sv(&mut self) -> Option<i64> {
        let u = self.uv()?;
        Some((u >> 1) as i64 ^ -((u & 1) as i64))
    }
}

/// An unsigned variable-length integer (7 bits at a time, the high bit marks a continuation)
pub fn put_uv(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A signed (zigzag) variable-length integer
pub fn put_sv(out: &mut Vec<u8>, v: i64) {
    put_uv(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_uv(out, b.len() as u64);
    out.extend_from_slice(b);
}

impl BuildCache {
    pub fn empty() -> BuildCache {
        BuildCache { data: vec![], index: FxMap::default() }
    }

    /// Reads the file (empty if it cannot be read or has a different format)
    pub fn open(path: &std::path::Path) -> BuildCache {
        let data = std::fs::read(path).unwrap_or_default();
        let mut c = BuildCache::empty();
        if data.len() < 8 || &data[..8] != MAGIC {
            return c;
        }
        let mut r = Rd { d: &data, pos: 8 };
        let Some(n) = r.uv() else { return c };
        let mut idx = FxMap::default();
        for _ in 0..n {
            let (Some(k), Some(len)) = (r.u64(), r.uv()) else { return c };
            let start = r.pos;
            if r.take(len as usize).is_none() {
                return c;
            }
            idx.insert(k, (start, start + len as usize));
        }
        c.index = idx;
        c.data = data;
        c
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn get(&self, key: Key) -> Option<&[u8]> {
        let &(s, e) = self.index.get(&(key as u64))?;
        Some(&self.data[s..e])
    }

    /// Writes the entries (to a temporary file, then rename). The number of bytes written
    /// (the temporary file's name is per process: if another zustc saving to the same output at the same time kept writing to the file after it was renamed,
    ///  the reader could accept a mix of the two writes; formal/tla/CacheSave.tla)
    pub fn save(path: &std::path::Path, entries: &[(Key, &[u8])]) -> Result<usize, String> {
        let mut out = MAGIC.to_vec();
        put_uv(&mut out, entries.len() as u64);
        for (k, b) in entries {
            out.extend_from_slice(&(*k as u64).to_le_bytes());
            put_bytes(&mut out, b);
        }
        // SAFETY: getpid takes no arguments and does not fail
        let tmp = format!("{}.{}.tmp", path.display(), unsafe { c_getpid() });
        if let Err(e) = std::fs::write(&tmp, &out) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{tmp}: {e}"));
        }
        let a = format!("{tmp}\0");
        let b = format!("{}\0", path.display());
        // SAFETY: both are NUL-terminated strings
        let r = unsafe { c_rename(a.as_bytes().as_ptr(), b.as_bytes().as_ptr()) };
        if r != 0 {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("{}: rename failed", path.display()));
        }
        Ok(out.len())
    }
}

/// A reference to a fn: an ordinary fn by name (tag 0), an instance by (original fn name, type arguments) (tag 2), an empty vtable slot (tag 6), a vtable's number entry (tag 7)
fn put_func(out: &mut Vec<u8>, id: u32, prog: &Program, insts: &Insts) {
    if id == u32::MAX {
        out.push(6);
        return;
    }
    // (a vtable's size or alignment entry: the number)
    if let Some(n) = crate::acg::vt_lit(id) {
        out.push(7);
        put_uv(out, n);
        return;
    }
    match insts.get(id) {
        None => {
            out.push(0);
            put_bytes(out, prog.names[id as usize].as_bytes());
        }
        Some(inst) => {
            out.push(2);
            put_bytes(out, prog.names[inst.base as usize].as_bytes());
            out.push(inst.args.len() as u8);
            for &t in &inst.args {
                crate::types_enc::put_ty(out, &prog.ty_syn(t));
            }
        }
    }
}

fn get_name<'a>(r: &mut Rd<'a>) -> Option<&'a str> {
    let n = r.uv()? as usize;
    let s = r.d.get(r.pos..r.pos + n)?;
    r.pos += n;
    std::str::from_utf8(s).ok()
}

/// The inverse of put_func (after the tag has been read). Instances are requested again from insts (pushed onto pending)
fn get_func(r: &mut Rd, tag: u8, prog: &Program, insts: &Mutex<Insts>) -> Option<u32> {
    if tag == 6 {
        return Some(u32::MAX);
    }
    if tag == 7 {
        return Some(crate::acg::VT_LIT | r.uv()? as u32);
    }
    let name = get_name(r)?;
    let id = *prog.index.get(name)?;
    if tag == 0 {
        return Some(id);
    }
    let na = r.u8()? as usize;
    let mut args = Vec::with_capacity(na);
    for _ in 0..na {
        let mut pos = r.pos;
        let ts = crate::types_enc::read_ty(r.d, &mut pos)?;
        r.pos = pos;
        args.push(prog.resolve(&ts).ok()?);
    }
    insts.lock().unwrap().request(prog, id, args).ok()
}

/// Writes the machine code + relocations + anonymous data + vtables + panic locations (integers as varints, offsets and lines as deltas from the previous one)
pub fn encode_blob(b: &Blob, prog: &Program, insts: &Insts, out: &mut Vec<u8>) {
    // (the low bit: the closures' facts follow at the end)
    put_uv(out, (b.align as u64) << 1 | !b.clo_facts.is_empty() as u64);
    put_bytes(out, &b.code);
    put_uv(out, b.relocs.len() as u64);
    let mut prev = 0u32;
    for r in &b.relocs {
        put_sv(out, r.offset as i64 - prev as i64);
        prev = r.offset;
        out.push(r.kind);
        put_sv(out, r.addend);
        match &r.target {
            Target::Func(id) => put_func(out, *id, prog, insts),
            Target::SelfOffset(o) => {
                out.push(1);
                put_uv(out, *o as u64);
            }
            Target::Data(k) => {
                out.push(3);
                put_uv(out, *k as u64);
            }
            // A static by name (its number changes with the order of the items)
            Target::Static(ci) => {
                out.push(4);
                put_bytes(out, prog.consts[*ci as usize].name.as_bytes());
            }
            Target::Vtable(k) => {
                out.push(5);
                put_uv(out, *k as u64);
            }
            Target::Sym(n) => {
                out.push(7);
                put_bytes(out, n.as_bytes());
            }
        }
    }
    put_uv(out, b.datas.len() as u64);
    for d in &b.datas {
        put_bytes(out, d);
    }
    put_uv(out, b.vtables.len() as u64);
    for vt in &b.vtables {
        put_uv(out, vt.len() as u64);
        for &f in vt {
            put_func(out, f, prog, insts);
        }
    }
    put_uv(out, b.traps.len() as u64);
    let (mut po, mut pl) = (0i64, 0i64);
    for &(o, info, m) in &b.traps {
        let (kind, line, col) = (info >> 27, (info >> 11) & 0xFFFF, info & 0x7FF);
        put_sv(out, o as i64 - po);
        po = o as i64;
        let m1 = if m == u32::MAX { 0 } else { m as u64 + 1 };
        put_uv(out, kind as u64 | (m1 << 4));
        put_sv(out, line as i64 - pl);
        pl = line as i64;
        put_uv(out, col as u64);
    }
    put_uv(out, b.line_base.map_or(0, |l| l as u64 + 1));
    // (what the lowering told the closures it makes: by the body's name)
    if b.clo_facts.is_empty() {
        return;
    }
    put_uv(out, b.clo_facts.len() as u64);
    for f in &b.clo_facts {
        put_bytes(out, prog.names[f.body as usize].as_bytes());
        out.push(f.kind_free as u8 | (f.rg.is_some() as u8) << 1);
        if let Some(rg) = &f.rg {
            put_uv(out, rg.codes.len() as u64);
            for c in &rg.codes {
                put_bytes(out, c);
            }
            put_uv(out, rg.caps.len() as u64);
            for c in &rg.caps {
                match c {
                    Some(c) => {
                        out.push(1);
                        put_bytes(out, c);
                    }
                    None => out.push(0),
                }
            }
            put_uv(out, rg.known.len() as u64);
            for &(l, s) in &rg.known {
                out.extend_from_slice(&[l, s]);
            }
            put_uv(out, rg.lts.len() as u64);
            for (n, c) in &rg.lts {
                put_bytes(out, n.as_bytes());
                out.push(*c);
            }
        }
    }
}

fn get_clo_fact(r: &mut Rd, prog: &Program) -> Option<CloFact> {
    let body = *prog.index.get(get_name(r)?)?;
    let fl = r.u8()?;
    let rg = if fl & 2 != 0 {
        let mut rg = CloRg::default();
        for _ in 0..r.uv()? {
            let n = r.uv()? as usize;
            rg.codes.push(r.take(n)?.to_vec());
        }
        for _ in 0..r.uv()? {
            rg.caps.push(match r.u8()? {
                0 => None,
                _ => {
                    let n = r.uv()? as usize;
                    Some(r.take(n)?.to_vec())
                }
            });
        }
        for _ in 0..r.uv()? {
            rg.known.push((r.u8()?, r.u8()?));
        }
        for _ in 0..r.uv()? {
            let n = get_name(r)?.to_string();
            rg.lts.push((n, r.u8()?));
        }
        Some(Box::new(rg))
    } else {
        None
    };
    Some(CloFact { body, kind_free: fl & 1 != 0, rg })
}

/// The inverse of encode_blob. Callee instances are requested again from insts
pub fn decode_blob(d: &[u8], prog: &Program, insts: &Mutex<Insts>) -> Option<Blob> {
    let mut r = Rd { d, pos: 0 };
    let a0 = r.uv()?;
    let align = (a0 >> 1) as u32;
    let n = r.uv()? as usize;
    let code = r.take(n)?.to_vec();
    let nr = r.uv()? as usize;
    let mut relocs = Vec::with_capacity(nr);
    let mut prev = 0i64;
    for _ in 0..nr {
        prev += r.sv()?;
        let offset = prev as u32;
        let kind = r.u8()?;
        let addend = r.sv()?;
        let tag = r.u8()?;
        let target = match tag {
            0 | 2 | 6 => Target::Func(get_func(&mut r, tag, prog, insts)?),
            1 => Target::SelfOffset(r.uv()? as u32),
            3 => Target::Data(r.uv()? as u32),
            4 => Target::Static(*prog.const_index.get(get_name(&mut r)?)?),
            5 => Target::Vtable(r.uv()? as u32),
            7 => Target::Sym(get_name(&mut r)?.to_string()),
            _ => return None,
        };
        relocs.push(BlobReloc { offset, kind, addend, target });
    }
    let nd = r.uv()? as usize;
    let mut datas = Vec::with_capacity(nd);
    for _ in 0..nd {
        let n = r.uv()? as usize;
        datas.push(r.take(n)?.to_vec());
    }
    let nv = r.uv()? as usize;
    let mut vtables = Vec::with_capacity(nv);
    for _ in 0..nv {
        let n = r.uv()? as usize;
        let mut vt = Vec::with_capacity(n);
        for _ in 0..n {
            let tag = r.u8()?;
            vt.push(get_func(&mut r, tag, prog, insts)?);
        }
        vtables.push(vt);
    }
    let nt = r.uv()? as usize;
    let mut traps = Vec::with_capacity(nt);
    let (mut po, mut pl) = (0i64, 0i64);
    for _ in 0..nt {
        po += r.sv()?;
        let km = r.uv()?;
        pl += r.sv()?;
        let col = r.uv()? as u32;
        let info = ((km & 0xF) as u32) << 27 | ((pl as u32) & 0xFFFF) << 11 | (col & 0x7FF);
        let m = (km >> 4) as u32;
        traps.push((po as u32, info, m.wrapping_sub(1)));
    }
    let line_base = match r.uv()? {
        0 => None,
        l => Some(l as u32 - 1),
    };
    // (the closures' facts go back to insts: their bodies are checked as the lowering, not made here, told them)
    let nf = if a0 & 1 != 0 { r.uv()? as usize } else { 0 };
    let mut clo_facts = Vec::with_capacity(nf);
    for _ in 0..nf {
        clo_facts.push(get_clo_fact(&mut r, prog)?);
    }
    if !clo_facts.is_empty() {
        let mut g = insts.lock().unwrap();
        g.give_facts(&clo_facts);
    }
    Some(Blob { align, code, relocs, datas, vtables, traps, line_base, code_ref: None, clo_facts })
}

#[cfg(test)]
mod save_tests {
    use super::BuildCache;

    /// save writes through a tmp file of this process's own and leaves none behind; open reads back what was saved.
    /// (Two processes saving at once each rename their own complete file: formal/tla/CacheSave.tla)
    #[test]
    fn save_then_open() {
        let dir = std::env::temp_dir().join(format!("build_cache_save_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.cache");
        // (a tmp file of the name every process used before: it is not ours, and is left alone)
        std::fs::write(dir.join("p.cache.tmp"), b"partial").unwrap();
        BuildCache::save(&path, &[(1, b"one".as_slice()), (2, b"two".as_slice())]).unwrap();
        let c = BuildCache::open(&path);
        assert_eq!(c.get(1), Some(b"one".as_slice()));
        assert_eq!(c.get(2), Some(b"two".as_slice()));
        let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["p.cache", "p.cache.tmp"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
