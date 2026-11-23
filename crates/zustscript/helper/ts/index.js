// zustscript-helper: the values a zustscript module passes between JS and wasm, in one stable byte format (the
// format's table is in the Rust crate zustscript_helper, helper/rust/src/lib.rs). zsc's glue calls `encode` for an
// argument and `decode` for a result; both walk the value by its schema, since the bytes carry no type tags.
//
// A schema: "i32" | "u32" | "i64" | "f32" | "f64" | "bool" | "str" | ["arr", schema] | ["rec", [[field, schema], ..]]

const enc = new TextEncoder();
const dec = new TextDecoder();

/** The bytes of `value` (a value of `schema`) */
export function encode(schema, value) {
  const w = { buf: new Uint8Array(64), view: null, n: 0 };
  w.view = new DataView(w.buf.buffer);
  put(w, schema, value);
  return w.buf.subarray(0, w.n);
}

/** The value of `schema` that `bytes` hold */
export function decode(schema, bytes) {
  const r = { view: new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength), bytes, i: 0 };
  return get(r, schema);
}

function room(w, n) {
  if (w.n + n <= w.buf.length) return;
  let len = w.buf.length * 2;
  while (len < w.n + n) len *= 2;
  const b = new Uint8Array(len);
  b.set(w.buf.subarray(0, w.n));
  w.buf = b;
  w.view = new DataView(b.buffer);
}

function put(w, s, v) {
  switch (s) {
    case "i32": room(w, 4); w.view.setInt32(w.n, v, true); w.n += 4; return;
    case "u32": room(w, 4); w.view.setUint32(w.n, v, true); w.n += 4; return;
    case "f32": room(w, 4); w.view.setFloat32(w.n, v, true); w.n += 4; return;
    case "i64": room(w, 8); w.view.setBigInt64(w.n, v, true); w.n += 8; return;
    case "f64": room(w, 8); w.view.setFloat64(w.n, v, true); w.n += 8; return;
    case "bool": room(w, 1); w.buf[w.n++] = v ? 1 : 0; return;
    case "str": {
      // (at most 3 UTF-8 bytes per UTF-16 unit)
      room(w, 4 + 3 * v.length);
      const { written } = enc.encodeInto(v, w.buf.subarray(w.n + 4));
      w.view.setUint32(w.n, written, true);
      w.n += 4 + written;
      return;
    }
  }
  if (s[0] === "arr") {
    room(w, 4);
    w.view.setUint32(w.n, v.length, true);
    w.n += 4;
    for (let i = 0; i < v.length; i++) put(w, s[1], v[i]);
    return;
  }
  // a record: its declared fields only, in order
  const fs = s[1];
  for (let i = 0; i < fs.length; i++) put(w, fs[i][1], v[fs[i][0]]);
}

function get(r, s) {
  let v;
  switch (s) {
    case "i32": v = r.view.getInt32(r.i, true); r.i += 4; return v;
    case "u32": v = r.view.getUint32(r.i, true); r.i += 4; return v;
    case "f32": v = r.view.getFloat32(r.i, true); r.i += 4; return v;
    case "i64": v = r.view.getBigInt64(r.i, true); r.i += 8; return v;
    case "f64": v = r.view.getFloat64(r.i, true); r.i += 8; return v;
    case "bool": return r.bytes[r.i++] !== 0;
    case "str": {
      const n = r.view.getUint32(r.i, true);
      v = dec.decode(r.bytes.subarray(r.i + 4, r.i + 4 + n));
      r.i += 4 + n;
      return v;
    }
  }
  if (s[0] === "arr") {
    const n = r.view.getUint32(r.i, true);
    r.i += 4;
    v = new Array(n);
    for (let k = 0; k < n; k++) v[k] = get(r, s[1]);
    return v;
  }
  v = {};
  const fs = s[1];
  for (let i = 0; i < fs.length; i++) v[fs[i][0]] = get(r, fs[i][1]);
  return v;
}
