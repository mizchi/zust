// the calls of tests/stdlib.zsc, their results as JSON (tests/examples.rs stdlib_same_in_js_and_wasm)
const m = await import(process.argv[2]);
const out = [m.arrays([5, 3, 8, 1]), m.arrays([]), m.names(), m.strings("  Hello, World  "), m.strings("h😀é"), m.strings(""),
  m.parse(["42", "-7", "+3", " 1", "1.5", "99999999999", "", "-0", "2147483647", "-2147483648", "2147483648"]),
  m.parsef(["1.5", "-2e3", ".5", "5.", "inf", "-Infinity", "NaN", " 1", "0x10", "", "e5", "1e", "1_0"]).map(String),
  [0.3, 1, 2.75, 7.1, -2.5].map((x) => m.floats(x).map(String)), [-7, 0, 1234567].map((i) => m.ints(i)), [-9n, 3n].map((i) => m.bigs(i).map(String))];
console.log(JSON.stringify(out));
