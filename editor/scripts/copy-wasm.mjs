// Copies the compiler built by `cargo build` into public/ (the grammar's wasm is written there by tree-sitter)
import { copyFileSync, mkdirSync, statSync } from "node:fs";
const what = process.argv[2];
mkdirSync("public", { recursive: true });
if (what === "zsc") {
  const from = "zsc-wasm/target/wasm32-unknown-unknown/release/zsc_wasm.wasm";
  copyFileSync(from, "public/zsc.wasm");
  console.log(`public/zsc.wasm (${statSync("public/zsc.wasm").size} bytes)`);
} else if (what === "grammar") {
  console.log(`public/tree-sitter-zustscript.wasm (${statSync("public/tree-sitter-zustscript.wasm").size} bytes)`);
}
