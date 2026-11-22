// core::arch::wasm32: constructors, splat, lanes, shuffles, bitwise ops, loads and stores (issue #30)
use std::arch::wasm32::*;

fn h(name: &str, v: v128) {
    println!(
        "{name}: {:08x} {:08x} {:08x} {:08x}",
        u32x4_extract_lane::<0>(v),
        u32x4_extract_lane::<1>(v),
        u32x4_extract_lane::<2>(v),
        u32x4_extract_lane::<3>(v)
    );
}

fn main() {
    // constructors and splats of every shape
    h("i8x16", i8x16(1, -2, 3, -4, 5, -6, 7, -8, 9, -10, 11, -12, 13, -14, 15, -128));
    h("u8x16", u8x16(255, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 200));
    h("i16x8", i16x8(1, -1, 300, -300, 32767, -32768, 7, 8));
    h("u16x8", u16x8(65535, 1, 2, 3, 40000, 5, 6, 7));
    h("i32x4", i32x4(1, -1, i32::MAX, i32::MIN));
    h("u32x4", u32x4(u32::MAX, 0, 0x8000_0000, 42));
    h("i64x2", i64x2(-1, 0x1234_5678_9abc_def0));
    h("u64x2", u64x2(u64::MAX, 7));
    h("f32x4", f32x4(1.0, -2.5, 0.0, -0.0));
    h("f64x2", f64x2(1.5, -1e300));
    h("splat8", i8x16_splat(-3));
    h("splat8u", u8x16_splat(250));
    h("splat16", i16x8_splat(-300));
    h("splat16u", u16x8_splat(60000));
    h("splat32", i32x4_splat(-7));
    h("splat32u", u32x4_splat(0xdead_beef));
    h("splat64", i64x2_splat(-2));
    h("splat64u", u64x2_splat(1 << 40));
    h("splatf32", f32x4_splat(3.25));
    h("splatf64", f64x2_splat(-0.125));

    // lanes
    let a = i8x16(1, -2, 3, -4, 5, -6, 7, -8, 9, -10, 11, -12, 13, -14, 15, -128);
    println!("lanes8: {} {} {} {}", i8x16_extract_lane::<1>(a), u8x16_extract_lane::<1>(a), i8x16_extract_lane::<15>(a), u8x16_extract_lane::<15>(a));
    let a16 = i16x8(1, -1, 300, -300, 32767, -32768, 7, 8);
    println!("lanes16: {} {} {} {}", i16x8_extract_lane::<1>(a16), u16x8_extract_lane::<1>(a16), i16x8_extract_lane::<5>(a16), u16x8_extract_lane::<3>(a16));
    let a32 = i32x4(1, -1, i32::MAX, i32::MIN);
    println!("lanes32: {} {} {} {}", i32x4_extract_lane::<1>(a32), u32x4_extract_lane::<1>(a32), i32x4_extract_lane::<3>(a32), u32x4_extract_lane::<2>(a32));
    let a64 = i64x2(-1, 0x1234_5678_9abc_def0);
    println!("lanes64: {} {} {}", i64x2_extract_lane::<0>(a64), u64x2_extract_lane::<0>(a64), i64x2_extract_lane::<1>(a64));
    let f = f32x4(1.0, -2.5, 0.0, -0.0);
    println!("lanesf32: {} {} {} {}", f32x4_extract_lane::<0>(f), f32x4_extract_lane::<1>(f), f32x4_extract_lane::<2>(f), f32x4_extract_lane::<3>(f));
    let d = f64x2(1.5, -1e300);
    println!("lanesf64: {} {}", f64x2_extract_lane::<0>(d), f64x2_extract_lane::<1>(d));
    h("replace8", i8x16_replace_lane::<3>(a, -1));
    h("replace8u", u8x16_replace_lane::<15>(a, 0x7f));
    h("replace16", i16x8_replace_lane::<0>(a16, -2));
    h("replace16u", u16x8_replace_lane::<7>(a16, 0xabcd));
    h("replace32", i32x4_replace_lane::<2>(a32, 5));
    h("replace32u", u32x4_replace_lane::<0>(a32, 0xffff_0000));
    h("replace64", i64x2_replace_lane::<1>(a64, -5));
    h("replace64u", u64x2_replace_lane::<0>(a64, 9));
    h("replacef32", f32x4_replace_lane::<1>(f, 8.0));
    h("replacef64", f64x2_replace_lane::<0>(d, 2.0));

    // shuffles and swizzles
    let x = u8x16(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
    let y = u8x16(16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31);
    h("shuffle8", i8x16_shuffle::<31, 0, 30, 1, 29, 2, 28, 3, 27, 4, 26, 5, 25, 6, 24, 7>(x, y));
    h("shuffle8u", u8x16_shuffle::<0, 0, 0, 0, 16, 16, 16, 16, 15, 15, 15, 15, 31, 31, 31, 31>(x, y));
    h("shuffle16", i16x8_shuffle::<7, 6, 5, 4, 8, 9, 10, 11>(x, y));
    h("shuffle16u", u16x8_shuffle::<0, 15, 1, 14, 2, 13, 3, 12>(x, y));
    h("shuffle32", i32x4_shuffle::<3, 4, 1, 6>(x, y));
    h("shuffle32u", u32x4_shuffle::<0, 0, 7, 7>(x, y));
    h("shuffle64", i64x2_shuffle::<1, 2>(x, y));
    h("shuffle64u", u64x2_shuffle::<3, 0>(x, y));
    h("swizzle", i8x16_swizzle(x, u8x16(15, 14, 13, 12, 0, 1, 2, 3, 16, 17, 200, 255, 4, 4, 4, 4)));
    h("swizzleu", u8x16_swizzle(y, i8x16(3, 2, 1, 0, -1, 15, 15, 15, 8, 9, 10, 11, 12, 13, 14, 15)));

    // bitwise
    let p = u32x4(0xff00_ff00, 0x0f0f_0f0f, 0xffff_ffff, 0);
    let q = u32x4(0xf0f0_f0f0, 0x00ff_00ff, 0x1234_5678, 0x8765_4321);
    h("not", v128_not(p));
    h("and", v128_and(p, q));
    h("andnot", v128_andnot(p, q));
    h("or", v128_or(p, q));
    h("xor", v128_xor(p, q));
    h("bitselect", v128_bitselect(p, q, u32x4(0xffff_0000, 0x00ff_ff00, 0, 0xffff_ffff)));
    println!("any_true: {} {} {}", v128_any_true(p), v128_any_true(u32x4_splat(0)), v128_any_true(u64x2(0, 1 << 63)));
    println!(
        "all_true: {} {} {} {} {}",
        i8x16_all_true(x),
        u8x16_all_true(y),
        i16x8_all_true(i16x8(1, 1, 1, 1, 1, 1, 1, 256)),
        i32x4_all_true(p),
        i64x2_all_true(i64x2(1, -1))
    );
    println!("all_true2: {} {} {}", u16x8_all_true(u16x8_splat(9)), u32x4_all_true(q), u64x2_all_true(u64x2(1, 0)));
    println!(
        "bitmask: {} {} {} {} {} {}",
        i8x16_bitmask(i8x16(-1, 0, -1, 0, 1, -128, 127, -2, 0, 0, 0, 0, 0, 0, 0, -1)),
        u8x16_bitmask(y),
        i16x8_bitmask(i16x8(-1, 0, 1, -2, 0, -32768, 0, -1)),
        i32x4_bitmask(i32x4(-1, 0, -5, 3)),
        i64x2_bitmask(i64x2(0, -1)),
        u32x4_bitmask(p)
    );
    println!("bitmask2: {} {}", u16x8_bitmask(u16x8_splat(0x8000)), u64x2_bitmask(u64x2(1 << 63, 1 << 63)));

    // memory
    let mut buf = [0u8; 64];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i * 7 + 3) as u8;
    }
    unsafe {
        let v = v128_load(buf.as_ptr().add(1) as *const v128);
        h("load", v);
        h("load8_splat", v128_load8_splat(buf.as_ptr().add(5)));
        h("load16_splat", v128_load16_splat(buf.as_ptr().add(6) as *const u16));
        h("load32_splat", v128_load32_splat(buf.as_ptr().add(7) as *const u32));
        h("load64_splat", v128_load64_splat(buf.as_ptr().add(9) as *const u64));
        h("load32_zero", v128_load32_zero(buf.as_ptr().add(3) as *const u32));
        h("load64_zero", v128_load64_zero(buf.as_ptr().add(11) as *const u64));
        h("load_ext8s", i16x8_load_extend_i8x8(buf.as_ptr().add(16) as *const i8));
        h("load_ext8u", i16x8_load_extend_u8x8(buf.as_ptr().add(16)));
        h("load_ext8u2", u16x8_load_extend_u8x8(buf.as_ptr().add(24)));
        h("load_ext16s", i32x4_load_extend_i16x4(buf.as_ptr().add(32) as *const i16));
        h("load_ext16u", i32x4_load_extend_u16x4(buf.as_ptr().add(32) as *const u16));
        h("load_ext32s", i64x2_load_extend_i32x2(buf.as_ptr().add(40) as *const i32));
        h("load_ext32u", u64x2_load_extend_u32x2(buf.as_ptr().add(40) as *const u32));
        h("load8_lane", v128_load8_lane::<3>(v, buf.as_ptr().add(60)));
        h("load16_lane", v128_load16_lane::<7>(v, buf.as_ptr().add(50) as *const u16));
        h("load32_lane", v128_load32_lane::<0>(v, buf.as_ptr().add(44) as *const u32));
        h("load64_lane", v128_load64_lane::<1>(v, buf.as_ptr().add(33) as *const u64));
        v128_store(buf.as_mut_ptr().add(40) as *mut v128, i32x4(-1, 2, -3, 4));
        v128_store8_lane::<5>(v, buf.as_mut_ptr().add(2));
        v128_store16_lane::<2>(v, buf.as_mut_ptr().add(4) as *mut u16);
        v128_store32_lane::<3>(v, buf.as_mut_ptr().add(8) as *mut u32);
        v128_store64_lane::<1>(v, buf.as_mut_ptr().add(13) as *mut u64);
    }
    let s: u32 = buf.iter().enumerate().map(|(i, &b)| (i as u32 + 1) * b as u32).sum();
    println!("buf: {s} {:?}", &buf[..24]);
}
