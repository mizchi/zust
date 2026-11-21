// Runs examples/kernels's functions (tests/examples.rs: on the @simd, the --no-simd and the --all-js builds).
// The f64 sums are printed rounded to 12 digits: @simd regroups them (lane by lane), the rest must match exactly.
const m = await import((await import("node:url")).pathToFileURL((await import("node:path")).resolve(process.argv[2])).href);
const out = [];
const p = (name, v) => out.push(name + " " + JSON.stringify(v));
const r12 = (x) => Number(x.toPrecision(12));
const a = Array.from({ length: 1001 }, (_, i) => Math.sin(i) * 3), b = Array.from({ length: 1001 }, (_, i) => Math.cos(i * 0.7));
p("dot", r12(m.dot(a, b)));
p("dot_small", [m.dot([1, 2, 3], [4, 5, 6]), m.dot([], [])]);
p("sum", r12(m.sum(a)));
p("axpy", m.axpy(2, [1, 2, 3, 4.5, 5], [10, 20, 30, 40, 50]));
p("axpy_exact", m.axpy(1.5, a, b).every((v, i) => v === 1.5 * a[i] + b[i]));
p("norms", m.norms([3, 5, 8], [4, 12, 15]));
p("affine", m.affine([1, -2, 3.5], 0.5, 1));
const ints = Array.from({ length: 1003 }, (_, i) => (i * 2654435761) | 0);
p("isum", [m.isum(ints), m.isum([2147483647, 1, 2, 3, 4])]);
p("imix", m.imix([1, 2, 3, 4, 5, 2147483647], [5, 4, 3, 2, 1, 0], 3));
let err;
try { m.dot([1, 2, 3], [1]); } catch (e) { err = e instanceof Error; }
p("short_array_error", err);
console.log(out.join("\n"));
