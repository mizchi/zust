// tests/js_values.zsc's JS values: a class, a fetch that fails or rejects, a function that throws, nullable results
globalThis.Counter = class { constructor(n) { this.count = n; this.label = null; } bump(by) { this.count += by; return this.count; } reset(to) { this.count = to ?? 0; } getLabel() { return this.label; } };
globalThis.fakeFetch = async (url) => {
  if (url === "down") throw new TypeError("fetch failed");
  return { ok: url !== "missing", status: url === "missing" ? 404 : 200, text: async () => "body of " + url, json: async () => { if (url === "bad") throw new SyntaxError("Unexpected token"); return { url }; } };
};
globalThis.risky = (n) => { if (n < 0) throw new RangeError("negative " + n); return n * 10; };
globalThis.findName = (id) => (id === 1 ? "one" : null);
globalThis.greet = (n) => (n === undefined ? "hello" : "hi " + n);
const m = await import(process.argv[2]);
console.log(m.urls());
// (the message of JSON.parse's SyntaxError differs between node versions)
const bad = m.roundtrip("{oops");
console.log(JSON.stringify([m.roundtrip('{"a":[1,2]}'), bad.TAG, typeof bad._0]));
console.log(m.counting());
console.log(JSON.stringify([await m.load("ok"), await m.load("missing"), await m.load("down"), await m.load_json("j"), await m.load_json("bad")]));
console.log(m.thrown());
console.log(m.base_of("/a/b/c.txt"), m.path_base("/x/y.md").base);
