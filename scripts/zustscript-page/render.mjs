// Render the public introduction, and Rust guide with Tree-sitter highlighting, before deployment.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, join, resolve } from "node:path";

const work = resolve(process.argv[2]);
const output = resolve(process.argv[3]);
const require = createRequire(join(work, "package.json"));
const MarkdownIt = require("markdown-it");
const { decodeHTML } = require("entities");
const scopes = new Map(Object.entries({
  zustscript: "zustscript", typescript: "ts", javascript: "js", bash: "bash", rust: "rust",
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
const pages = new Map([
  ["introduction.md", { destination: output, label: "zustscript 入門" }],
  ["rust-costs.md", { destination: join(dirname(output), "rust-costs.html"), label: "Rust から見る変換コスト" }],
]);
const template = readFileSync(new URL("./template.html", import.meta.url), "utf8");
const plain = token => token.children.map(child => child.content).join("");
mkdirSync(dirname(output), { recursive: true });

for (const [file, { destination }] of pages) {
  const source = readFileSync(new URL("../../crates/zustscript/docs/" + file, import.meta.url), "utf8");
  const tokens = md.parse(source, {});
  const lead = tokens.findIndex(t => t.type === "paragraph_open");
  if (tokens[0]?.tag !== "h1" || lead < 0) throw new Error(file + " must begin with an H1 and a lead paragraph");
  for (const token of tokens) {
    for (const child of token.children ?? []) {
      const target = pages.get(child.attrGet("href"));
      if (target) child.attrSet("href", basename(target.destination));
    }
  }
  const articleLinks = Array.from(pages, ([sourceFile, page]) => {
    const href = md.utils.escapeHtml(basename(page.destination));
    const current = sourceFile === file ? ' aria-current="page"' : "";
    return `<a href="${href}"${current}>${md.utils.escapeHtml(page.label)}</a>`;
  });
  const slots = {
    articleLinks: articleLinks.join("\n      "),
    title: md.utils.escapeHtml(plain(tokens[1])),
    description: md.utils.escapeHtml(plain(tokens[lead + 1])),
    article: md.renderer.render(tokens, md.options, {}).trimEnd(),
    current: destination === output ? "page" : "location",
  };
  writeFileSync(destination, template.replace(/\{\{(title|description|article|current|articleLinks)\}\}/g, (_, key) => slots[key]));
}
