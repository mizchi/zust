// @simd / @simd(relaxed) vs --no-simd (zustc) vs --all-js: node tests/ml_bench.mjs <simd> <no-simd> <js>
const imp = async (p) => import((await import("node:url")).pathToFileURL((await import("node:path")).resolve(p)).href);
const [s, n, j] = [await imp(process.argv[2]), await imp(process.argv[3]), await imp(process.argv[4])];
const time = (f, reps) => {
  for (let k = 0; k < 3; k++) f();
  const t = performance.now();
  for (let k = 0; k < reps; k++) f();
  return ((performance.now() - t) / reps).toFixed(2);
};
const rnd = () => Math.random() - 0.5;
const [M, N] = [512, 512];
const w = new Float32Array(M * N).map(rnd), x = new Float32Array(N).map(rnd), b = new Float32Array(M).map(rnd);
const big = new Float32Array(1_000_000).map(rnd), big2 = new Float32Array(1_000_000).map(rnd);
const rows = [
  ["dense 512×512 (W copied in per call)", (m) => () => m.dense(w, x, b, M, N), (m) => () => m.dense_fma(w, x, b, M, N), 20],
  ["relu 1e6", (m) => () => m.relu(big), (m) => () => m.relu_relaxed(big), 20],
  ["saxpy 1e6 (relaxed: fused)", null, (m) => () => m.saxpy(0.5, big, big2), 20],
  ["sumsq 1e6", (m) => () => m.sumsq(big), null, 20],
];
console.log(`node ${process.version}\n| kernel | @simd | @simd(relaxed) | --no-simd (zustc) | JS |\n|---|---|---|---|---|`);
for (const [name, plain, relaxed, reps] of rows) {
  const any = plain || relaxed;
  const cell = (mk, m) => (mk ? time(mk(m), reps) + " ms" : "-");
  console.log(`| ${name} | ${cell(plain, s)} | ${cell(relaxed, s)} | ${time(any(n), reps)} ms | ${time(any(j), reps)} ms |`);
}
