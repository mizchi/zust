import { loadCompiler, CompilerCrash, type Diagnostic, type Output } from "./zsc";
import { loadHighlighter, type Highlighter } from "./highlight";
import { run, type LogLine } from "./preview";
import zscUrl from "/zsc.wasm?url";
import grammarUrl from "/tree-sitter-zustscript.wasm?url";

const samples = import.meta.glob("./samples/*.zsc", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const SAMPLE_ORDER = ["counter", "basics", "numeric", "effects"];

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const source = $<HTMLTextAreaElement>("source");
const highlightEl = $<HTMLPreElement>("highlight");
const gutter = $<HTMLDivElement>("gutter");
const status = $<HTMLDivElement>("status");
const diagList = $<HTMLUListElement>("diagnostics");
const outJs = $<HTMLPreElement>("out-js");
const outDts = $<HTMLPreElement>("out-dts");
const frame = $<HTMLDivElement>("frame");
const consoleEl = $<HTMLPreElement>("console");
const runPanel = document.querySelector<HTMLElement>('[data-panel="run"]')!;
const sampleSel = $<HTMLSelectElement>("sample");
const autorun = $<HTMLInputElement>("autorun");

let compile: ((src: string) => Output) | null = null;
let highlighter: Highlighter | null = null;
let last: Output | null = null;
let ranJs: string | null = null;

// ---- the source: the URL's hash, else the last session, else the first example
const names = Object.keys(samples)
  .map((p) => p.replace(/^.*\/(.*)\.zsc$/, "$1"))
  .sort((a, b) => rank(a) - rank(b));
function rank(n: string) {
  const i = SAMPLE_ORDER.indexOf(n);
  return i === -1 ? 99 : i;
}
for (const n of names) sampleSel.append(new Option(`${n}.zsc`, n));
sampleSel.append(new Option("(your code)", ""));

function initial(): string {
  const h = location.hash.match(/^#code=(.*)$/);
  if (h) {
    try {
      return decodeHash(h[1]);
    } catch {
      /* fall through */
    }
  }
  try {
    const saved = localStorage.getItem("zsc-playground:source");
    if (saved) return saved;
  } catch {
    /* no storage */
  }
  return samples[`./samples/${names[0]}.zsc`];
}
source.value = initial();
syncSample();

function syncSample() {
  const hit = names.find((n) => samples[`./samples/${n}.zsc`] === source.value);
  sampleSel.value = hit ?? "";
}

sampleSel.addEventListener("change", () => {
  if (!sampleSel.value) return;
  source.value = samples[`./samples/${sampleSel.value}.zsc`];
  source.setSelectionRange(0, 0);
  $<HTMLDivElement>("editor").scrollTo(0, 0);
  changed(true);
});

// ---- the editor
function render(diags: Diagnostic[]) {
  const src = source.value;
  const classes = highlighter ? highlighter.update(src).classes : [];
  const marks: (string | undefined)[] = new Array(src.length);
  const lineStarts = [0];
  for (let i = 0; i < src.length; i++) if (src.charCodeAt(i) === 10) lineStarts.push(i + 1);
  const lineSev = new Map<number, string>();
  for (const d of diags) {
    const [s, e] = range(src, lineStarts, d);
    const cls = d.severity === "error" ? "d-error" : "d-warning";
    for (let i = s; i < e; i++) if (marks[i] !== "d-error") marks[i] = cls;
    if (lineSev.get(d.line) !== "err") lineSev.set(d.line, d.severity === "error" ? "err" : "warn");
  }
  const out: string[] = [];
  let i = 0;
  while (i < src.length) {
    const c = classes[i];
    const m = marks[i];
    let j = i + 1;
    while (j < src.length && classes[j] === c && marks[j] === m && src.charCodeAt(j) !== 10) j++;
    const text = esc(src.slice(i, j));
    const cls = [c, m].filter(Boolean).join(" ");
    out.push(cls ? `<span class="${cls}">${text}</span>` : text);
    i = j;
  }
  // (a trailing newline needs a character after it to take up a line)
  highlightEl.innerHTML = out.join("") + "\n";
  gutter.innerHTML = lineStarts.map((_, k) => {
    const sev = lineSev.get(k + 1);
    return sev ? `<span class="${sev}">${k + 1}</span>` : `${k + 1}`;
  }).join("\n");
}

/** The source range a diagnostic underlines: the token at its position (zsc counts columns in characters) */
function range(src: string, lineStarts: number[], d: Diagnostic): [number, number] {
  const ls = lineStarts[Math.max(0, Math.min(d.line - 1, lineStarts.length - 1))] ?? 0;
  const le = src.indexOf("\n", ls) === -1 ? src.length : src.indexOf("\n", ls);
  let s = ls;
  for (let k = 0; k < d.col && s < le; k++) s += (src.codePointAt(s) ?? 0) > 0xffff ? 2 : 1;
  let e = s;
  if (/[\p{L}\p{N}_]/u.test(src[s] ?? "")) {
    while (e < le && /[\p{L}\p{N}_]/u.test(src[e])) e++;
  } else if (src[s] === '"') {
    e = src.indexOf('"', s + 1);
    e = e === -1 || e > le ? le : e + 1;
  } else {
    e = Math.min(s + 1, le);
  }
  if (e === s) {
    // (at the end of the line: the last character before it)
    return [Math.max(ls, s - 1), Math.max(s, ls + 1)];
  }
  return [s, e];
}

function esc(s: string) {
  return s.replace(/[&<>]/g, (c) => (c === "&" ? "&amp;" : c === "<" ? "&lt;" : "&gt;"));
}

function showDiagnostics(diags: Diagnostic[]) {
  diagList.replaceChildren(
    ...diags.map((d) => {
      const li = document.createElement("li");
      li.className = d.severity;
      li.innerHTML = `<span class="sev">${d.severity}</span><span class="at">${d.line}:${d.col + 1}</span>${esc(d.message)}`;
      li.addEventListener("click", () => {
        const src = source.value;
        const starts = [0];
        for (let i = 0; i < src.length; i++) if (src.charCodeAt(i) === 10) starts.push(i + 1);
        const [s, e] = range(src, starts, d);
        source.focus();
        source.setSelectionRange(s, e);
        const lineTop = (d.line - 1) * 20;
        const ed = $<HTMLDivElement>("editor");
        if (lineTop < ed.scrollTop || lineTop > ed.scrollTop + ed.clientHeight - 40) ed.scrollTo({ top: lineTop - ed.clientHeight / 3 });
      });
      return li;
    }),
  );
}

// ---- compile on every edit, run when it compiles (auto-run) or on ▶
let timer = 0;
function changed(now = false) {
  clearTimeout(timer);
  if (now) recompile();
  else {
    // (the highlighting follows the keys at once, the compiler a moment later)
    render(last?.diagnostics ?? []);
    timer = window.setTimeout(recompile, 120);
  }
  try {
    localStorage.setItem("zsc-playground:source", source.value);
  } catch {
    /* no storage */
  }
}

function recompile() {
  syncSample();
  if (!compile) {
    render([]);
    return;
  }
  try {
    last = compile(source.value);
  } catch (e) {
    if (!(e instanceof CompilerCrash)) throw e;
    // (the instance is unusable after a trap: no edit reaches it while a fresh one loads, and the fresh one
    // compiles the source only when it changed since the crash, so a source that crashes zsc is not retried in a loop)
    last = { ok: false, js: "", dts: "", diagnostics: [], ms: 0 };
    const crashed = source.value;
    compile = null;
    status.textContent = `zsc crashed: ${e.message} (edit the source to try again)`;
    status.className = "status bad";
    void loadCompiler(zscUrl).then((c) => {
      compile = c;
      if (source.value !== crashed) recompile();
    });
    render([]);
    return;
  }
  render(last.diagnostics);
  showDiagnostics(last.diagnostics);
  const errs = last.diagnostics.filter((d) => d.severity === "error").length;
  const warns = last.diagnostics.length - errs;
  if (last.ok) {
    outJs.textContent = last.js;
    outDts.textContent = last.dts;
    status.textContent = `compiled in ${last.ms.toFixed(1)} ms · ${last.js.length.toLocaleString()} chars of JS${warns ? ` · ${warns} warning(s)` : ""}`;
    status.className = "status";
    if (autorun.checked && last.js !== ranJs) start();
  } else {
    status.textContent = `${errs} error(s)`;
    status.className = "status bad";
  }
}

function start() {
  if (!last?.ok) return;
  ranJs = last.js;
  consoleEl.replaceChildren();
  runPanel.classList.remove("rendered");
  run(frame, last.js, log, (e) => {
    if (e === "render") runPanel.classList.add("rendered");
  });
}

function log(l: LogLine) {
  const div = document.createElement("div");
  div.className = l.kind;
  div.textContent = l.text;
  consoleEl.append(div);
  consoleEl.scrollTop = consoleEl.scrollHeight;
}

source.addEventListener("input", () => changed());
// Tab indents; Esc then Tab leaves the editor (no keyboard trap)
let tabLeaves = false;
source.addEventListener("keydown", (e) => {
  const leave = tabLeaves;
  tabLeaves = e.key === "Escape";
  if (e.key === "Tab" && leave) return;
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
    e.preventDefault();
    recompile();
    start();
  } else if (e.key === "Tab" && !e.ctrlKey && !e.metaKey && !e.altKey) {
    e.preventDefault();
    indent(e.shiftKey);
  } else if (e.key === "Enter" && !e.shiftKey && !e.altKey) {
    // keep the indentation of the line
    e.preventDefault();
    const s = source.selectionStart;
    const ls = source.value.lastIndexOf("\n", s - 1) + 1;
    const ind = source.value.slice(ls, s).match(/^[ \t]*/)![0];
    const more = /[{([]\s*$/.test(source.value.slice(ls, s)) ? "  " : "";
    document.execCommand("insertText", false, "\n" + ind + more);
  }
});

/** Tab / Shift+Tab: two spaces in, or the selected lines in and out (through execCommand, to keep undo) */
function indent(out: boolean) {
  const v = source.value;
  const s = source.selectionStart;
  const e = source.selectionEnd;
  if (s === e && !out) {
    document.execCommand("insertText", false, "  ");
    return;
  }
  const ls = v.lastIndexOf("\n", s - 1) + 1;
  const le = v.indexOf("\n", e - (e > s && v[e - 1] === "\n" ? 1 : 0));
  const end = le === -1 ? v.length : le;
  const lines = v.slice(ls, end).split("\n");
  const moved = lines.map((l) => (out ? l.replace(/^ {1,2}/, "") : "  " + l));
  source.setSelectionRange(ls, end);
  document.execCommand("insertText", false, moved.join("\n"));
  const d0 = moved[0].length - lines[0].length;
  source.setSelectionRange(Math.max(ls, s + d0), ls + moved.join("\n").length);
}

$("run").addEventListener("click", () => {
  recompile();
  start();
});

$("share").addEventListener("click", async () => {
  const url = `${location.href.replace(/#.*$/, "")}#code=${encodeHash(source.value)}`;
  history.replaceState(null, "", url);
  const b = $("share");
  try {
    await navigator.clipboard.writeText(url);
    b.textContent = "link copied";
  } catch {
    b.textContent = "link in the address bar";
  }
  setTimeout(() => (b.textContent = "share"), 1600);
});

function encodeHash(s: string) {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function decodeHash(h: string) {
  const bin = atob(h.replace(/-/g, "+").replace(/_/g, "/"));
  return new TextDecoder().decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
}

// ---- tabs
for (const b of document.querySelectorAll<HTMLButtonElement>(".tabs button")) {
  b.addEventListener("click", () => {
    for (const o of document.querySelectorAll<HTMLButtonElement>(".tabs button")) o.setAttribute("aria-selected", String(o === b));
    for (const p of document.querySelectorAll<HTMLElement>("[data-panel]")) p.hidden = p.dataset.panel !== b.dataset.tab;
  });
}

// ---- load the compiler and the grammar
render([]);
Promise.all([
  loadHighlighter(grammarUrl).then((h) => {
    highlighter = h;
    render(last?.diagnostics ?? []);
  }),
  loadCompiler(zscUrl).then((c) => {
    compile = c;
    recompile();
  }),
]).catch((e) => {
  status.textContent = `failed to load: ${e} (build the wasm files with \`npm run build:wasm\`)`;
  status.className = "status bad";
});
