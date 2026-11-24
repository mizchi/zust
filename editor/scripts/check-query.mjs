// Prints the highlight captures of a file (to check queries/highlights.scm): node scripts/check-query.mjs FILE [N]
import { readFileSync } from "node:fs";
import { Parser, Language, Query } from "web-tree-sitter";
await Parser.init();
const lang = await Language.load("public/tree-sitter-zustscript.wasm");
const q = new Query(lang, readFileSync("tree-sitter-zustscript/queries/highlights.scm", "utf8"));
const p = new Parser(); p.setLanguage(lang);
const src = readFileSync(process.argv[2], "utf8");
const t = p.parse(src);
for (const c of q.captures(t.rootNode).slice(0, +process.argv[3] || 80)) console.log(c.name.padEnd(22), JSON.stringify(c.node.text.slice(0, 30)));
