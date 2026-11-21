// Runs examples/numeric's functions: tests/examples.rs runs it on the wasm build and on the --all-js build and
// compares (the same source must give the same results either way).
const m = await import(process.argv[2]);
const out = [];
const show = (v) => (Array.isArray(v) ? "[" + v.map(show).join(",") + "]" : typeof v === "bigint" ? v + "n" : String(v));
const p = (name, v) => out.push(name + " " + show(v));
p("dot", m.dot([1, 2, 3], [4, 5, 6.5]));
p("total$i32", m.total$i32([2147483647, 1, 5]));
p("total$u32", m.total$u32([4294967295, 2]));
p("total$i64", m.total$i64([9007199254740993n, 9007199254740993n]));
p("total$f64", m.total$f64([0.1, 0.2]));
p("mean", m.mean([1, 2, 4]));
p("mean0", m.mean([]));
p("normalize", m.normalize([1, -4, 2]));
p("normalize0", m.normalize([0, 0]));
p("evens_squared", m.evens_squared([1, 2, 3, 4, 46341, 46342, -6]));
p("escape", [m.escape(0, 0, 50), m.escape(1, 1, 50), m.escape(-0.75, 0.1, 50)]);
p("collatz", [m.collatz_steps(27n), m.collatz_steps(9007199254740993n)]);
p("lcg", m.lcg(12345, 1000));
p("edge_cases", [m.edge_cases(40000), m.edge_cases(-1), m.edge_cases(0)]);
let caught;
try { m.dot([1, 2], [1]); } catch (e) { caught = e instanceof Error ? "error" : "?"; }
p("out_of_bounds", caught);
console.log(out.join("\n"));
