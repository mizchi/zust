// @simd vs --no-simd (zustc, scalar) vs --all-js on the same source:
//   node tests/kernels_bench.mjs <simd build> <no-simd build> <js build>
const imp = async (p) => import((await import("node:url")).pathToFileURL((await import("node:path")).resolve(p)).href);
const [s, n, j] = [await imp(process.argv[2]), await imp(process.argv[3]), await imp(process.argv[4])];
const time = (f, reps) => {
  for (let k = 0; k < 3; k++) f();
  const t = performance.now();
  for (let k = 0; k < reps; k++) f();
  return ((performance.now() - t) / reps).toFixed(2);
};
const N = 1_000_000;
const a = Array.from({ length: N }, (_, i) => Math.sin(i)), b = Array.from({ length: N }, (_, i) => Math.cos(i));
const ints = Array.from({ length: N }, (_, i) => (i * 2654435761) | 0);
const fa = new Float64Array(a);
console.log("dot exact vs simd:", j.dot(a, b), s.dot(a, b), "diff", Math.abs(j.dot(a, b) - s.dot(a, b)));
const rows = [
  ["dot 1e6", (m) => () => m.dot(a, b), 10],
  ["sum 1e6", (m) => () => m.sum(a), 10],
  ["axpy 1e6 (returns an array)", (m) => () => m.axpy(1.5, a, b), 5],
  ["norms 1e6 (sqrt)", (m) => () => m.norms(a, b), 5],
  ["isum 1e6 i32", (m) => () => m.isum(ints), 10],
  ["dot over a Float64Array 1e6", (m) => () => m.dot(fa, fa), 10],
];
console.log("| kernel | @simd | --no-simd (zustc) | JS |\n|---|---|---|---|");
for (const [name, mk, reps] of rows) console.log(`| ${name} | ${time(mk(s), reps)} ms | ${time(mk(n), reps)} ms | ${time(mk(j), reps)} ms |`);
