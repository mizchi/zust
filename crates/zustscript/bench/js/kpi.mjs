// JS-target KPIs: output size and speed of `zsc --all-js` builds.
//
//   node bench/js/kpi.mjs [--zsc PATH] [--esbuild PATH] [--save FILE] [--against FILE] [--quick]
//
// Size: every examples/*.zsc and bench/js/workloads.zsc is compiled with --all-js, then each module is bundled
// with esbuild (--bundle --minify, react kept external) and gzipped. "raw" is zsc's own output, "min" and "gz" the
// bundle, which is what an app ships. Without esbuild only raw and raw+gz are reported.
// Speed: each `pub fn w_*` of bench/js/workloads.zsc runs a fixed number of times after a warm-up; the median of
// several runs is reported, with the workload's result so a change in behaviour shows up.
// --save writes the numbers as JSON; --against compares with a saved file (the change is marked when past 5% for
// speed, any change for size).
import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
// (an option given without its operand is an error, not the default: `--save` at the end must not drop the report)
const opt = (k, d) => {
  const i = args.indexOf(k);
  if (i < 0) return d;
  if (i + 1 >= args.length) {
    console.error(`${k} needs a value`);
    process.exit(2);
  }
  return args[i + 1];
};
const zsc = opt("--zsc", join(root, "target/release/zsc"));
const esbuild = opt("--esbuild", process.env.ESBUILD || "esbuild");
const quick = args.includes("--quick");
// (extra zsc flags, e.g. --inline-runtime to measure the runtime written into each module)
const extra = args.includes("--zsc-flags") ? opt("--zsc-flags").split(" ") : [];
// (the baseline is read before the output directory is cleaned: it may be a file saved there)
const against = opt("--against");
let base = null;
if (against !== undefined) {
  try {
    base = JSON.parse(readFileSync(against, "utf8"));
  } catch (e) {
    console.error(`--against ${against}: ${e.message}`);
    process.exit(2);
  }
}
const save = opt("--save");
const out = join(root, "target/js-kpi");
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

const hasEsbuild = (() => {
  try {
    execFileSync(esbuild, ["--version"], { stdio: "pipe" });
    return true;
  } catch {
    return false;
  }
})();

// ---- size
const sources = [
  ...readdirSync(join(root, "examples")).filter((f) => f.endsWith(".zsc")).map((f) => join(root, "examples", f)),
  join(root, "bench/js/workloads.zsc"),
];
const size = {};
for (const src of sources) {
  const name = src.split("/").pop().replace(/\.zsc$/, "");
  const dir = join(out, name);
  execFileSync(zsc, [src, "-o", dir, "--all-js", ...extra], { stdio: "pipe" });
  const js = join(dir, `${name}.js`);
  const raw = readFileSync(js);
  const row = { raw: raw.length, raw_gz: gzipSync(raw, { level: 9 }).length };
  if (hasEsbuild) {
    const min = execFileSync(esbuild, [js, "--bundle", "--minify", "--format=esm", "--external:react", "--external:react/*", "--external:node:*"], { stdio: ["pipe", "pipe", "pipe"] });
    row.min = min.length;
    row.gz = gzipSync(min, { level: 9 }).length;
  }
  size[name] = row;
}
const total = (k) => Object.values(size).reduce((s, r) => s + (r[k] ?? 0), 0);
// all the modules in one app: what an app ships when it uses several modules (shared helpers count once)
let all = null;
if (hasEsbuild) {
  const dir = join(out, "all");
  mkdirSync(dir, { recursive: true });
  const names = Object.keys(size);
  for (const name of names) execFileSync(zsc, [sources.find((s) => s.endsWith(`/${name}.zsc`)), "-o", dir, "--all-js", ...extra], { stdio: "pipe" });
  writeFileSync(join(dir, "entry.js"), names.map((n, i) => `export * as m${i} from "./${n}.js";`).join("\n") + "\n");
  const min = execFileSync(esbuild, [join(dir, "entry.js"), "--bundle", "--minify", "--format=esm", "--external:react", "--external:react/*", "--external:node:*"], { stdio: ["pipe", "pipe", "pipe"] });
  all = { min: min.length, gz: gzipSync(min, { level: 9 }).length };
}
size.TOTAL = { raw: total("raw"), raw_gz: total("raw_gz"), ...(hasEsbuild ? { min: total("min"), gz: total("gz") } : {}) };
if (all) size["ALL (one bundle)"] = { raw: "", raw_gz: "", ...all };

// ---- speed
const m = await import(pathToFileURL(join(out, "workloads/workloads.js")).href);
// [argument, calls per run]. w_effects keeps the small argument of the first baseline (then, generator-based handlers
// overflowed the stack around 1000 operations in one `handle`), so the numbers stay comparable
const loads = { w_effects: [400, 500], w_records: [200, 1], w_enums: [500, 1], w_numeric: [2_000_000, 1], w_strings: [2_000, 1] };
const runs = quick ? 3 : 7;
const speed = {};
for (const [name, [n, calls]] of Object.entries(loads)) {
  const f = m[name];
  f(n); // warm-up
  const times = [];
  let result;
  for (let k = 0; k < runs; k++) {
    const t = performance.now();
    for (let c = 0; c < calls; c++) result = f(n);
    times.push(performance.now() - t);
  }
  times.sort((a, b) => a - b);
  speed[name] = { ms: +times[runs >> 1].toFixed(2), result: String(result) };
}

// ---- report
const report = { node: process.version, esbuild: hasEsbuild, size, speed };

const pct = (a, b) => (b ? `${a >= b ? "+" : ""}${(((a - b) / b) * 100).toFixed(1)}%` : "");
const cols = hasEsbuild ? ["raw", "min", "gz"] : ["raw", "raw_gz"];
console.log(`## JS target (node ${process.version}${hasEsbuild ? ", esbuild --minify" : ", no esbuild: raw only"})\n`);
console.log(`| module | ${cols.join(" | ")} |${base ? " change (" + cols.at(-1) + ") |" : ""}`);
console.log(`|---|${cols.map(() => "---:").join("|")}|${base ? "---:|" : ""}`);
for (const [name, r] of Object.entries(size)) {
  const b = base?.size?.[name];
  const k = cols.at(-1);
  const mark = b && b[k] !== r[k] ? ` ${pct(r[k], b[k])}` : b ? " =" : "";
  console.log(`| ${name} | ${cols.map((c) => r[c]).join(" | ")} |${base ? mark + " |" : ""}`);
}
console.log(`\n| workload | ms | result |${base ? " change |" : ""}`);
console.log(`|---|---:|---|${base ? "---:|" : ""}`);
for (const [name, r] of Object.entries(speed)) {
  const b = base?.speed?.[name];
  let mark = "";
  if (b) {
    const p = (r.ms - b.ms) / b.ms;
    mark = ` ${pct(r.ms, b.ms)}${Math.abs(p) > 0.05 ? (p > 0 ? " ⚠️" : " ✅") : ""}${b.result !== r.result ? " (RESULT CHANGED)" : ""} |`;
  }
  console.log(`| ${name} | ${r.ms} | ${r.result} |${mark}`);
}
if (save) writeFileSync(save, JSON.stringify(report, null, 2) + "\n");
