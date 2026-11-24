// Parses the zustscript examples with the grammar (as the playground loads it) and reports ERROR / MISSING nodes
import { readFileSync, readdirSync } from "node:fs";
import { Parser, Language } from "web-tree-sitter";
await Parser.init();
const lang = await Language.load("public/tree-sitter-zustscript.wasm");
const p = new Parser();
p.setLanguage(lang);
const dirs = ["../crates/zustscript/examples", "../crates/zustscript/tests", "../crates/zustscript/tests/modules", "src/samples"];
let bad = 0;
for (const d of dirs) {
  let files = [];
  try { files = readdirSync(d).filter((f) => f.endsWith(".zsc")); } catch { continue; }
  for (const f of files) {
    const src = readFileSync(`${d}/${f}`, "utf8");
    const tree = p.parse(src);
    const errs = [];
    const walk = (n) => {
      if (n.isError || n.isMissing) { errs.push(n); return; }
      for (const c of n.children) walk(c);
    };
    if (tree.rootNode.hasError) walk(tree.rootNode);
    if (errs.length) {
      bad++;
      console.log(`${d}/${f}: ${errs.length} error(s)`);
      for (const e of errs.slice(0, 5)) console.log(`  ${e.startPosition.row + 1}:${e.startPosition.column + 1} ${e.isMissing ? "missing " + e.type : "ERROR"}: ${JSON.stringify(src.slice(e.startIndex, Math.min(e.endIndex, e.startIndex + 60)))}`);
    }
  }
}
console.log(bad ? `${bad} file(s) with errors` : "grammar: every example parses");
process.exit(bad ? 1 : 0);
