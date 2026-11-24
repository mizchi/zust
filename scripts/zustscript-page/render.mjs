// Render the public introduction, including its Tree-sitter spans, before deployment.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";

const work = resolve(process.argv[2]);
const output = resolve(process.argv[3]);
const require = createRequire(join(work, "package.json"));
const MarkdownIt = require("markdown-it");
const { decodeHTML } = require("entities");
const scopes = new Map(Object.entries({
  zustscript: "zustscript", typescript: "ts", javascript: "js", bash: "bash",
}));
const config = join(work, "config.json");
writeFileSync(config, JSON.stringify({ "parser-directories": [join(work, "node_modules")] }));

function highlight(code, language) {
  const scope = scopes.get(language);
  if (!scope || !code) return ""; // markdown-it escapes plain text and unregistered languages.
  const snippet = join(work, "example." + language);
  writeFileSync(snippet, code);
  const rendered = execFileSync("tree-sitter", [
    "highlight", "--html", "--css-classes", "--config-path", config, "--scope", "source." + scope, snippet,
  ], { cwd: work, encoding: "utf8" });
  const colored = Array.from(rendered.matchAll(/<td class=line>([\s\S]*?)<\/td>/g), m => m[1]).join("");
  if (decodeHTML(colored.replace(/<\/?span\b[^>]*>/g, "")) !== code) {
    throw new Error("Tree-sitter changed the text of a " + language + " code block");
  }
  return colored;
}

const md = new MarkdownIt({ highlight });
const source = readFileSync(new URL("../../crates/zustscript/docs/introduction.md", import.meta.url), "utf8");
const tokens = md.parse(source, {});
const heading = tokens.findIndex(t => t.type === "heading_open" && t.tag === "h1");
const lead = tokens.findIndex(t => t.type === "paragraph_open");
if (heading !== 0 || lead < 0) throw new Error("Start the introduction with an H1 and an introductory paragraph");
const plain = token => token.children.map(child => child.content).join("");
const slots = {
  title: md.utils.escapeHtml(plain(tokens[heading + 1])),
  description: md.utils.escapeHtml(plain(tokens[lead + 1])),
  article: md.renderer.render(tokens, md.options, {}).trimEnd(),
};
const template = readFileSync(new URL("./template.html", import.meta.url), "utf8");
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, template.replace(/\{\{(title|description|article)\}\}/g, (_, key) => slots[key]));
