// Runs examples/ml's kernels: tests/examples.rs runs it on the @simd, --no-simd and --all-js builds. Elementwise
// results (relu, non-relaxed) are printed exactly; sums (and relaxed ones, fused) rounded to 5 digits (f32).
const m = await import((await import("node:url")).pathToFileURL((await import("node:path")).resolve(process.argv[2])).href);
const out = [];
const p = (name, v) => out.push(name + " " + JSON.stringify(v));
const r5 = (xs) => Array.from(xs, (x) => Number(x.toPrecision(5)));
// a deterministic "random" f32 matrix
let seed = 7;
const rnd = () => ((seed = (Math.imul(seed, 1103515245) + 12345) | 0) >>> 8) / (1 << 24) - 0.5;
const [hidden, inputs, outputs] = [37, 50, 5];
const w1 = new Float32Array(hidden * inputs).map(rnd), b1 = new Float32Array(hidden).map(rnd);
const w2 = new Float32Array(outputs * hidden).map(rnd), b2 = new Float32Array(outputs).map(rnd);
const x = new Float32Array(inputs).map(rnd);
const y = m.dense(w1, x, b1, hidden, inputs);
p("dense_is_f32array", y instanceof Float32Array && y.length === hidden);
p("dense", r5(y).slice(0, 8));
p("dense_fma", r5(m.dense_fma(w1, x, b1, hidden, inputs)).slice(0, 8));
p("relu", Array.from(m.relu(new Float32Array([-1, 0.5, -0, 2, -3.25, 7, 0.125]))));
p("relu_relaxed", r5(m.relu_relaxed(new Float32Array([-1, 0.5, 2, -3.25, 7, 0.125]))));
p("saxpy", r5(m.saxpy(0.5, new Float32Array([1, 2, 3, 4, 5]), new Float32Array([10, 20, 30, 40, 50]))));
p("sumsq", Number(m.sumsq(x).toPrecision(5)));
p("mlp", r5(m.mlp(w1, b1, w2, b2, x, hidden, inputs, outputs)));
let err;
try { m.dense(w1, x, b1, hidden + 1, inputs); } catch (e) { err = e instanceof RangeError || e instanceof Error; }
p("bounds_error", err);
console.log(out.join("\n"));
