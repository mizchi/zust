# zustscript playground

A page to write [zustscript](../crates/zustscript) in the browser: the editor highlights it with tree-sitter, zsc
(compiled to wasm) transpiles it on every edit, and the preview runs the result.

- **editor**: a textarea over the text that [web-tree-sitter](https://github.com/tree-sitter/tree-sitter/tree/master/lib/binding_web)
  colors with [the grammar](tree-sitter-zustscript/grammar.js) and its
  [highlights query](tree-sitter-zustscript/queries/highlights.scm) (parsed incrementally as you type). zsc's
  errors and warnings are underlined, and listed under the editor (click one to go to it).
- **compiler**: [zsc-wasm](zsc-wasm/src/lib.rs) builds crates/zustscript's own modules (not a copy) for
  `wasm32-unknown-unknown`, as `zsc --all-js --inline-runtime`.
  - zustc does not run in the browser, so every `@wasm` / `@simd` fn stays in the JS output: the same source, run
    as JS.
  - A Rust `wasm fn` cannot become JS. It is reported as a warning, and calling it throws.
  - `import type` has no TS files to read here.
- **preview**: the module runs in a sandboxed iframe (a new one per run) and calls `pub fn main`.
  - Its console output and its result are shown.
  - When `main` returns JSX, the element is rendered with React 19 from esm.sh, through an import map. Other bare
    imports (`extern "pkg" fn ..`) load from esm.sh too.
- **JS / .d.ts**: zsc's output.

The source is kept in localStorage. **share** puts it in the URL (`#code=…`).

## Build

You need Rust with the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`) and Node 20+.

```sh
npm install
npm run build:wasm   # zsc → public/zsc.wasm, the grammar → public/tree-sitter-zustscript.wasm
npm run dev          # or: npm run build (static files in dist/, servable from any path)
npm test             # every example parses with the grammar, compiles, and its main runs
```

`build:grammar` uses tree-sitter-cli, which downloads wasi-sdk the first time it builds a grammar to wasm. The wasm
files and tree-sitter's generated parser are build outputs and are not checked in.

Add an example by putting a `.zsc` file in [src/samples](src/samples).
