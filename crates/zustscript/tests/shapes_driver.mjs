// Runs examples/shapes's functions (tests/examples.rs: on the wasm and the --all-js builds, which must agree)
const m = await import(process.argv[2]);
const out = [];
const p = (name, v) => out.push(name + " " + JSON.stringify(v, (_, x) => (typeof x === "bigint" ? x + "n" : x)));
const v = (x, y) => ({ x, y });
const shapes = [{ TAG: "Circle", _0: v(1, 2), _1: 3 }, { TAG: "Rect", _0: v(0, 0), _1: v(4, -2) }, { TAG: "Dot", _0: v(5, 5) }];
p("area", shapes.map(m.area));
p("center", shapes.map(m.center));
p("grow", shapes.map((s) => m.grow(s, 2)));
const xs = [0, 3, 10, 2.5], ys = [0, 4, 1, 2.5];
p("nearest", [m.nearest(xs, ys, v(2, 2), "Exact"), m.nearest(xs, ys, v(9, 9), "Fast"), m.nearest([], [], v(0, 0), "Fast")]);
p("collatz_max", [m.collatz_max(10000n), m.collatz_max(1n)]);
console.log(out.join("\n"));
