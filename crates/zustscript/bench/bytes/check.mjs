// Bundle a control and the optional imports. Exercise both adapters on the same Rust Wasm.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const esbuild = process.env.ESBUILD || "esbuild";
const cwd = fileURLToPath(new URL("../../helper/ts/", import.meta.url));
const imports = 'import { asBytes, readBytes, writeBytes } from "zustscript-helper/bytes";';
const rows = {};
function measure(name, bytes) {
  rows[name] = { bytes: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length };
}
function bundle(name, source) {
  const js = execFileSync(esbuild, ["--bundle", "--minify", "--format=esm"], { cwd, input: source });
  measure(name, js);
  return js;
}

const control = "export const size = view => view.byteLength;";
const plain = bundle("JS control", control);
assert.deepEqual(bundle("JS unused import", imports + control), plain);
bundle("JS byte view only", 'export { asBytes } from "zustscript-helper/bytes";');

function adapter(before, after) {
  return `export function run(wasm, src) {
    const n = src.byteLength;
    if (n > 16) throw new RangeError("fixture capacity");
    const p = wasm.BUFFER.value;
    ${before}
    wasm.invert(n);
    return ${after};
  }`;
}
const direct = bundle("Wasm adapter, handwritten", adapter(
  "new Uint8Array(wasm.memory.buffer, p, n).set(new Uint8Array(src.buffer, src.byteOffset, n));",
  "new Uint8Array(wasm.memory.buffer, p, n).slice()",
));
const library = bundle("Wasm adapter, helper", imports + adapter(
  "writeBytes(wasm.memory, p, src);", "readBytes(wasm.memory, p, n)",
));

const dir = mkdtempSync(join(tmpdir(), "zustscript-bytes-"));
try {
  const wasmPath = join(dir, "kernel.wasm");
  const kernel = fileURLToPath(new URL("./kernel.rs", import.meta.url));
  execFileSync("rustc", [kernel, "--edition=2024", "--target=wasm32-unknown-unknown",
    "--crate-type=cdylib", "-Copt-level=z", "-Cpanic=abort", "-Cstrip=symbols", "-o", wasmPath]);
  const wasm = readFileSync(wasmPath);
  measure("Rust Wasm (both adapters)", wasm);
  const { instance } = await WebAssembly.instantiate(wasm);
  for (const js of [direct, library]) {
    const { run } = await import("data:text/javascript;base64," + js.toString("base64"));
    const input = Uint8Array.of(11, 0, 254, 255, 22);
    const output = run(instance.exports, new DataView(input.buffer, 1, 3));
    assert.deepEqual([...run(instance.exports, input.subarray(2, 4))], [1, 0]);
    assert.deepEqual([...run(instance.exports, input.subarray(5))], []);
    instance.exports.memory.grow(1);
    assert.deepEqual([...output], [255, 1, 0]);
    assert.deepEqual([...input], [11, 0, 254, 255, 22]);
  }
  console.table(rows);
} finally {
  rmSync(dir, { recursive: true, force: true });
}
