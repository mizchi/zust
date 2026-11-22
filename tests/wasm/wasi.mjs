// Runs a wasm32-wasip1 module (rustc's build of a tests/wasm/simd program: the oracle for zustc's module) under
// node's WASI, with no preopened directories and no environment: node tests/wasm/wasi.mjs x.wasm
import { WASI } from "node:wasi";
import fs from "node:fs";

process.removeAllListeners("warning");
const wasi = new WASI({ version: "preview1", args: [process.argv[2]], env: {}, returnOnExit: true });
const m = await WebAssembly.compile(fs.readFileSync(process.argv[2]));
const i = await WebAssembly.instantiate(m, wasi.getImportObject());
process.exitCode = wasi.start(i);
