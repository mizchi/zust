// JS (--all-js) vs wasm (@wasm) on the same source: node tests/numeric_bench.mjs <wasm build> <js build>
const w = await import(process.argv[2]);
const j = await import(process.argv[3]);
const time = (f, reps) => {
  f();
  const t = performance.now();
  let r;
  for (let k = 0; k < reps; k++) r = f();
  return [(performance.now() - t) / reps, r];
};
const n = 1_000_000;
const a = Array.from({ length: n }, (_, i) => (i % 100) / 7);
const b = Array.from({ length: n }, (_, i) => (i % 13) / 3);
const grid = (m) => () => {
  let s = 0;
  for (let y = 0; y < 120; y++) for (let x = 0; x < 160; x++) s += m.escape(-2.2 + x * 0.02, -1.2 + y * 0.02, 200);
  return s;
};
const rows = [
  ["dot, 1e6 f64 (copied into wasm per call)", (m) => () => m.dot(a, b), 5],
  ["total$f64, 1e6", (m) => () => m.total$f64(a), 5],
  ["mandelbrot 160x120, max 200 (one boundary call per point)", grid, 3],
  ["lcg, 1e6 u32 steps", (m) => () => m.lcg(1, 1_000_000), 5],
  ["collatz_steps over 1..20000 (i64)", (m) => () => { let s = 0; for (let k = 1n; k < 20000n; k++) s += m.collatz_steps(k); return s; }, 2],
];
for (const [name, mk, reps] of rows) {
  const [tw, rw] = time(mk(w), reps);
  const [tj, rj] = time(mk(j), reps);
  console.log(`${name}: wasm ${tw.toFixed(1)} ms, js ${tj.toFixed(1)} ms${String(rw) === String(rj) ? "" : " (DIFFERENT RESULTS)"}`);
}
