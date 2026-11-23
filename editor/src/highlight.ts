// Syntax highlighting with tree-sitter: the grammar of tree-sitter-zustscript/ and its queries/highlights.scm
import { Parser, Language, Query, Edit, type Tree, type Point } from "web-tree-sitter";
import treeSitterWasm from "web-tree-sitter/web-tree-sitter.wasm?url";
import highlights from "../tree-sitter-zustscript/queries/highlights.scm?raw";

export type Highlighter = {
  /** parses `src` (incrementally, from the previous text) and returns a class per UTF-16 unit */
  update(src: string): { classes: (string | undefined)[]; tree: Tree };
};

export async function loadHighlighter(grammarUrl: string): Promise<Highlighter> {
  await Parser.init({ locateFile: () => treeSitterWasm });
  const lang = await Language.load(grammarUrl);
  const parser = new Parser();
  parser.setLanguage(lang);
  const query = new Query(lang, highlights);
  let text = "";
  let tree: Tree | null = null;

  return {
    update(src) {
      if (tree) {
        const edit = diff(text, src);
        if (edit) tree.edit(edit);
      }
      const next = parser.parse(src, tree ?? undefined);
      if (!next) throw new Error("tree-sitter: parse failed");
      tree?.delete();
      tree = next;
      text = src;
      // the first pattern that captures a unit colors it (the specific patterns come first in highlights.scm)
      const classes: (string | undefined)[] = new Array(src.length);
      const caps = query.captures(tree.rootNode).sort((a, b) => a.patternIndex - b.patternIndex);
      for (const c of caps) {
        const cls = "hl-" + c.name.replaceAll(".", "-");
        for (let i = c.node.startIndex; i < c.node.endIndex; i++) classes[i] ??= cls;
      }
      return { classes, tree };
    },
  };
}

/** The edit from `a` to `b` (one changed range: a common prefix and suffix) */
function diff(a: string, b: string) {
  if (a === b) return null;
  let s = 0;
  const min = Math.min(a.length, b.length);
  while (s < min && a.charCodeAt(s) === b.charCodeAt(s)) s++;
  let ea = a.length;
  let eb = b.length;
  while (ea > s && eb > s && a.charCodeAt(ea - 1) === b.charCodeAt(eb - 1)) {
    ea--;
    eb--;
  }
  return new Edit({
    startIndex: s,
    oldEndIndex: ea,
    newEndIndex: eb,
    startPosition: point(a, s),
    oldEndPosition: point(a, ea),
    newEndPosition: point(b, eb),
  });
}

function point(s: string, i: number): Point {
  let row = 0;
  let last = -1;
  for (let k = s.indexOf("\n"); k !== -1 && k < i; k = s.indexOf("\n", k + 1)) {
    row++;
    last = k;
  }
  return { row, column: i - last - 1 };
}
