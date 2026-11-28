//! Serialization of type syntax (TySyn) and summaries (Summary) (for the cache). Split out of front.rs: since it does not depend on syn,
//! The session can use it too, and zustc itself can compile it.

use crate::enc::{Enc, Rd};
use crate::types::TYS;
use crate::types::*;

impl Enc for TySyn {
    fn enc(&self, out: &mut Vec<u8>) {
        match self {
            TySyn::Prim(Ty::Vec(b)) => {
                out.push(35);
                (*b as u64).enc(out);
            }
            TySyn::Prim(t) => out.push(TYS.iter().position(|x| x == t).unwrap() as u8),
            TySyn::Ref(m, i) => {
                out.push(20 + *m as u8);
                (**i).enc(out);
            }
            TySyn::Named(n) => {
                out.push(22);
                n.enc(out);
            }
            TySyn::App(n, a) => {
                out.push(23);
                n.enc(out);
                a.enc(out);
            }
            TySyn::Tuple(a) => {
                out.push(24);
                a.enc(out);
            }
            TySyn::Array(i, ArrLen::Lit(n)) => {
                out.push(25);
                (**i).enc(out);
                n.enc(out);
            }
            TySyn::Array(i, ArrLen::Const(c)) => {
                out.push(26);
                (**i).enc(out);
                c.enc(out);
            }
            TySyn::Slice(i) => {
                out.push(27);
                (**i).enc(out);
            }
            TySyn::Str => out.push(28),
            TySyn::Ptr(m, i) => {
                out.push(29 + *m as u8);
                (**i).enc(out);
            }
            TySyn::Dyn(n) => {
                out.push(31);
                n.enc(out);
            }
            TySyn::FnPtr(v) => {
                out.push(32);
                v.enc(out);
            }
            TySyn::Assoc(b, n) => {
                out.push(33);
                (**b).enc(out);
                n.enc(out);
            }
            TySyn::Infer => out.push(34),
        }
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some(match r.u8()? {
            c @ 0..18 => TySyn::Prim(TYS[c as usize]),
            c @ (20 | 21) => TySyn::Ref(c == 21, Box::new(TySyn::dec(r)?)),
            22 => TySyn::Named(String::dec(r)?),
            23 => TySyn::App(String::dec(r)?, Vec::dec(r)?),
            24 => TySyn::Tuple(Vec::dec(r)?),
            25 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Lit(u64::dec(r)?)),
            26 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Const(String::dec(r)?)),
            27 => TySyn::Slice(Box::new(TySyn::dec(r)?)),
            28 => TySyn::Str,
            c @ (29 | 30) => TySyn::Ptr(c == 30, Box::new(TySyn::dec(r)?)),
            31 => TySyn::Dyn(String::dec(r)?),
            32 => TySyn::FnPtr(Vec::dec(r)?),
            33 => TySyn::Assoc(Box::new(TySyn::dec(r)?), String::dec(r)?),
            34 => TySyn::Infer,
            35 => TySyn::Prim(Ty::Vec(u16::try_from(u64::dec(r)?).ok()?)),
            _ => return None,
        })
    }
}

impl Enc for VariantKind {
    fn enc(&self, out: &mut Vec<u8>) {
        out.push(*self as u8)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        [VariantKind::Unit, VariantKind::Tuple, VariantKind::Struct]
            .get(r.u8()? as usize)
            .copied()
    }
    fn is_empty_val(&self) -> bool {
        *self == VariantKind::Unit
    }
}

crate::enc_struct!(VariantSyn {
    name,
    kind,
    fields,
    discr,
    discr_src
});
crate::enc_struct!(SigSyn {
    params,
    ret,
    symbol,
    is_extern,
    is_main,
    impl_of,
    has_self,
    generics,
    trait_of,
    has_body,
    impl_trait,
    self_ty,
    export,
    panic_handler,
    ret_from,
    is_unsafe,
    regions,
    preds,
    own_unsized,
});
crate::enc_struct!(RSigSyn { lts, slots, outlives, ty_outlives, outer, self_slots, derived, derived_ty });
crate::enc_struct!(RImplSyn { lts, self_slots, outlives, ty_outlives, free, wf_pairs, wf_vague, trait_lts, self_paths });
crate::enc_struct!(Summary {
    fns,
    structs,
    unions,
    unit,
    reprs,
    const_fns,
    const_fn_tys,
    adt_err,
    impl_self,
    enums,
    trait_decl,
    trait_generics,
    impl_trait,
    derives,
    adt_generics,
    adt_bounds,
    adt_unsized,
    adt_defaults,
    impl_generics,
    impl_self_ty,
    impl_self_path,
    consts,
    statics,
    closures,
    assocs,
    aliases,
    alias_fixed_lts,
    const_regions,
    where_types,
    impl_trait_args,
    impl_trait_lts,
    assoc_bounds,
    hdr_regions,
    adt_regions,
    assoc_regions,
    ref_impl_fns,
    ref_impl_bad,
    ref_impl_unit_args,
});
crate::enc_struct!(RAdtSyn { lts, fields, outlives, ty_outlives });
crate::enc_struct!(AssocSyn {
    name,
    self_ty,
    generics,
    value,
});
crate::enc_struct!(ClosureSyn {
    name,
    generics,
    fields,
    sig,
    by_move,
    moved,
    ret_ann,
    n_parent,
    fake_reads,
    deref,
    mutated,
});


impl Enc for (String, TySyn, String, bool) {
    fn enc(&self, out: &mut Vec<u8>) {
        self.0.enc(out);
        self.1.enc(out);
        self.2.enc(out);
        self.3.enc(out);
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some((
            String::dec(r)?,
            TySyn::dec(r)?,
            String::dec(r)?,
            bool::dec(r)?,
        ))
    }
}

/// For writing an instance's type arguments into the cache
pub fn put_ty(out: &mut Vec<u8>, t: &TySyn) {
    t.enc(out)
}

/// Reads one TySyn from d[*pos..]
pub fn read_ty(d: &[u8], pos: &mut usize) -> Option<TySyn> {
    let mut r = Rd { d, pos: *pos };
    let t = TySyn::dec(&mut r);
    *pos = r.pos;
    t
}

impl Summary {
    pub fn encode(&self, out: &mut Vec<u8>) {
        self.enc(out)
    }

    pub fn decode(d: &[u8]) -> Option<Summary> {
        Summary::dec(&mut Rd::new(d))
    }
}

