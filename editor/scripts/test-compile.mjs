// Compiles the playground's examples (and crates/zustscript/examples) with the compiler the page loads, and runs
// the `main` of each one that does not need React, as the preview would
import { readFileSync, readdirSync } from "node:fs";

const { instance } = await WebAssembly.instantiate(readFileSync("public/zsc.wasm"), {});
const x = instance.exports;
function compile(src) {
  const b = new TextEncoder().encode(src);
  const p = x.zsc_alloc(b.length);
  new Uint8Array(x.memory.buffer, p, b.length).set(b);
  const n = x.zsc_compile(p, b.length);
  x.zsc_free(p, b.length);
  return JSON.parse(new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.zsc_result(), n)));
}

let bad = 0;
const check = (ok, what) => {
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
};
for (const d of ["src/samples", "../crates/zustscript/examples"]) {
  for (const f of readdirSync(d).filter((f) => f.endsWith(".zsc")).sort()) {
    const src = readFileSync(`${d}/${f}`, "utf8");
    const r = compile(src);
    if (d !== "src/samples" && /^@wasm\s/m.test(src)) {
      check(!r.ok && r.diagnostics.some((e) => e.message.includes("requires the Rust backend")), `${f}: Rust target required`);
      continue;
    }
    check(r.ok && r.js.length > 0 && r.dts.length > 0, `${d}/${f} compiles${r.ok ? "" : ": " + JSON.stringify(r.diagnostics)}`);
    if (!r.ok || r.js.includes('from "react')) continue;
    const m = await import("data:text/javascript;base64," + Buffer.from(r.js).toString("base64"));
    if (typeof m.main !== "function") continue;
    const log = console.log;
    console.log = () => {};
    let err = null;
    try {
      m.main();
    } catch (e) {
      err = e;
    } finally {
      console.log = log;
    }
    check(!err, `${d}/${f}: main() runs${err ? ": " + err : ""}`);
  }
}
// the errors come with their positions
const e = compile("export fn f() -> i32 {\n  \"no\"\n}\n");
check(!e.ok && e.diagnostics[0]?.line === 2 && e.diagnostics[0]?.severity === "error", `a type error is reported at 2:${e.diagnostics[0]?.col}`);
// a Rust `wasm fn` cannot run here: a warning, and a stub that throws
const w = compile("export wasm fn f(x: i32) -> i32 { x }\n");
check(w.ok && w.diagnostics.some((d) => d.severity === "warning") && w.js.includes("export function f()"), "a `wasm fn` is a warning and a stub");

console.log(bad ? `${bad} failure(s)` : "compile: all ok");
process.exit(bad ? 1 : 0);
