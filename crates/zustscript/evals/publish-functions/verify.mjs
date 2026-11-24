// Independently rebuild a submission in a new directory and exercise its public JS API.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const [task, source, compiler, backend, alternateModel] = process.argv.slice(2);
if (!["status", "energy", "aspect"].includes(task) || !source || !compiler || !backend) {
  throw new Error("usage: node verify.mjs status|energy|aspect answer.zsc zsc zustc [model.ts]");
}
const dir = mkdtempSync(join(tmpdir(), "zustscript-verify-"));
const checks = [];
function check(name, test) {
  try {
    test();
    checks.push({ name, passed: true });
  } catch (error) {
    checks.push({ name, passed: false, message: error.message });
  }
}
try {
  copyFileSync(source, join(dir, "answer.zsc"));
  const model = alternateModel ?? new URL(`./cases/${task}.ts`, import.meta.url);
  if (task !== "energy") copyFileSync(model, join(dir, "model.ts"));
  const build = spawnSync(resolve(compiler), ["answer.zsc", "--zustc", resolve(backend), "-o", "out"], {
    cwd: dir, encoding: "utf8",
  });
  if (build.error) throw build.error;
  check("compile", () => assert.equal(build.status, 0, build.stderr));
  if (build.status === 0) {
    const out = join(dir, "out");
    const mod = await import(pathToFileURL(join(out, "answer.js")));
    const dts = readFileSync(join(out, "answer.d.ts"), "utf8");
    const zsc = readFileSync(source, "utf8").replace(/\/\/[^\n]*|\/\*[\s\S]*?\*\//g, "");
    if (task === "status") {
      for (const [name, input, expected] of [
        ["waiting", { state: "waiting" }, "準備中"],
        ["shipped", { state: "shipped", code: "A17" }, "便 A17"],
        ["empty code", { state: "shipped", code: "" }, "便 "],
        ["Unicode code", { state: "shipped", code: "北-2" }, "便 北-2"],
      ]) check(name, () => assert.equal(mod.label(input), expected));
      check("export spelling", () => assert.match(zsc, /\bexport\s+fn\s+label\b/));
      check("private helper", () => {
        assert.match(zsc, /\bfn\s+format_code\b/);
        assert.ok((zsc.match(/\bformat_code\s*\(/g) ?? []).length >= 2);
        assert.ok(!("format_code" in mod) && !dts.includes("function format_code("));
      });
      check("original TS type", () => {
        assert.match(dts, /import type \{[^}]*\bShipment\b[^}]*\} from ["']\.\.\/model\.js["']/);
        assert.match(dts, /export declare function label\(\w+: Shipment\): string/);
      });
    } else if (task === "aspect") {
      for (const [name, input, expected] of [
        ["hidden", { kind: "hidden" }, 0],
        ["wide", { kind: "rect", height: 3, width: 12 }, 4],
        ["tall, different JS key order", { width: 3, kind: "rect", height: 12 }, 0.25],
        ["fractional dimensions", { kind: "rect", width: 1.5, height: 0.5 }, 3],
        ["zero height", { kind: "rect", height: 0, width: 8 }, 0],
        ["zero width", { kind: "rect", height: 8, width: 0 }, 0],
      ]) check(name, () => assert.equal(mod.aspect(input), expected));
      check("export spelling", () => assert.match(zsc, /\bexport\s+fn\s+aspect\b/));
      check("original TS type", () => {
        assert.match(dts, /import type \{[^}]*\bViewport\b[^}]*\} from ["']\.\.\/model\.js["']/);
        assert.match(dts, /export declare function aspect\(\w+: Viewport\): f64/);
      });
    } else {
      for (const [name, values, total] of [
        ["empty", [], 0], ["zero", [0, -0], 0], ["mixed signs", [3, -4], 25],
        ["fractional", [-0.5, 1.5], 2.5],
      ]) check(name, () => {
        const xs = new Float32Array(values);
        assert.equal(mod.energy(xs), total);
        assert.equal(mod.has_signal(xs), total > 0);
      });
      check("export spelling", () => {
        for (const name of ["energy", "has_signal"]) assert.match(zsc, new RegExp("\\bexport\\s+(?:wasm\\s+)?fn\\s+" + name + "\\b"));
      });
      check("typed JS API", () => {
        assert.match(dts, /export declare function energy\(\w+: Float32Array\): f32/);
        assert.match(dts, /export declare function has_signal\(\w+: Float32Array\): boolean/);
      });
      check("wasm energy and JS has_signal", () => {
        const file = ["answer.wasm", "answer.simd.wasm"].find(f => existsSync(join(out, f)));
        assert.ok(file, "expected a compiled Wasm module");
        const wasm = new WebAssembly.Module(readFileSync(join(out, file)));
        const names = WebAssembly.Module.exports(wasm).map(x => x.name);
        assert.ok(names.some(n => /(?:^|_)energy$/.test(n)), names.join(", "));
        assert.ok(!names.some(n => /(?:^|_)has_signal$/.test(n)));
        assert.match(fnBody(zsc, "has_signal") ?? "", /\benergy\s*\(/);
      });
    }
  }
} catch (error) {
  checks.push({ name: "load or run", passed: false, message: error.message });
} finally {
  rmSync(dir, { recursive: true, force: true });
}
const passed = checks.every(check => check.passed);

// The body of `fn name(..) { .. }`, braces balanced (an `if` / `else` inside it included; not those in strings and
// comments), or null
function fnBody(source, name) {
  const head = new RegExp("\\bfn\\s+" + name + "\\s*\\(").exec(source);
  if (!head) return null;
  const open = source.indexOf("{", head.index);
  if (open < 0) return null;
  let depth = 0;
  for (let i = open; i < source.length; i++) {
    const c = source[i];
    // (a string's braces and a comment's are not the block's: `let marker = "{";`)
    if (c === '"') {
      for (i++; i < source.length && source[i] !== '"'; i++) if (source[i] === "\\") i++;
    } else if (c === "/" && source[i + 1] === "/") {
      while (i < source.length && source[i] !== "\n") i++;
    } else if (c === "{") depth++;
    else if (c === "}" && --depth === 0) return source.slice(open + 1, i);
  }
  return null;
}

console.log(JSON.stringify({ task, passed, checks }, null, 2));
process.exitCode = passed ? 0 : 1;
