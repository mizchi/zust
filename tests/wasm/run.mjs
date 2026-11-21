// Runs a wasm module that zustc built for wasm32-unknown-unknown, as the sandbox's host.
//
//   node tests/wasm/run.mjs <module.wasm> [args...]
//
// The module gets only what this host hands it as imports of module `env`:
// - `write` to fd 1 and 2 (any other fd fails), `putchar`;
// - `open` / `read` / `close` of one virtual file, `/proc/self/cmdline` (the program's arguments: the prelude
//   reads std::env::args from it); every other path fails;
// - `exit`, `_exit`, `abort` (they end the run with that code);
// - `sigaction` and `fflush` as no-ops, `getenv` (no variables: always null);
// - math functions from `Math`, and `strtod` / `strtof` / `memmem` (pure functions over the module's memory).
// Nothing else: no file system, no network, no clock, no environment. A module that imports anything else is not
// run (exit 126, naming the imports).
//
// Exit codes: the program's own; 101 for a trap at `unreachable` (a panic, or a failed check: zustc's code traps
// there); 139 for a memory access out of bounds; 134 for other traps (the call stack exhausted, ...).
import fs from "node:fs";

const [, , path, ...args] = process.argv;
if (!path) {
  console.error("usage: node tests/wasm/run.mjs <module.wasm> [args...]");
  process.exit(2);
}
const mod = new WebAssembly.Module(fs.readFileSync(path));

class Exit {
  constructor(code) {
    this.code = code;
  }
}
let memory;
const bytes = () => new Uint8Array(memory.buffer);
const view = () => new DataView(memory.buffer);
const cstr = (p) => {
  const b = bytes();
  let e = Number(p);
  while (e < b.length && b[e] !== 0) e++;
  return Buffer.from(b.subarray(Number(p), e)).toString("latin1");
};

const stdio = [];
const flushStdio = () => {
  if (stdio.length > 0) fs.writeSync(1, Buffer.from(stdio.splice(0)));
};
const files = new Map();
let nextFd = 3;
const cmdline = Buffer.from(["main", ...args].map((a) => a + "\0").join(""));

// C's round: halfway cases away from zero
const cround = (x) => (x < 0 ? -Math.round(-x) : Math.round(x));
const unary = { sin: Math.sin, cos: Math.cos, tan: Math.tan, asin: Math.asin, acos: Math.acos, atan: Math.atan,
  sinh: Math.sinh, cosh: Math.cosh, tanh: Math.tanh, asinh: Math.asinh, acosh: Math.acosh, atanh: Math.atanh,
  exp: Math.exp, exp2: (x) => 2 ** x, expm1: Math.expm1, log: Math.log, log2: Math.log2, log10: Math.log10,
  log1p: Math.log1p, cbrt: Math.cbrt, round: cround };
const binary = { pow: Math.pow, atan2: Math.atan2, hypot: Math.hypot, fmod: (a, b) => a % b };
const math = {};
for (const [n, f] of Object.entries(unary)) {
  math[n] = f;
  math[n + "f"] = (x) => Math.fround(f(x));
}
for (const [n, f] of Object.entries(binary)) {
  math[n] = f;
  math[n + "f"] = (a, b) => Math.fround(f(a, b));
}

// strtod: the longest prefix that is a decimal float (or inf / nan); *end = after it
const FLOAT = /^[ \t\n\r\f\v]*[+-]?(?:inf(?:inity)?|nan|(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)/i;
const strtod = (s, end) => {
  const text = cstr(s);
  const m = FLOAT.exec(text);
  let len = 0;
  let v = 0;
  if (m) {
    len = m[0].length;
    const t = m[0].trim().toLowerCase();
    const neg = t.startsWith("-");
    const u = t.replace(/^[+-]/, "");
    v = u.startsWith("inf") ? Infinity : u === "nan" ? NaN : Number(u);
    if (neg) v = -v;
  }
  if (end !== 0n) view().setBigInt64(Number(end), s + BigInt(len), true);
  return v;
};

const env = {
  write(fd, p, n) {
    if (fd !== 1 && fd !== 2) return -1n;
    fs.writeSync(fd, bytes().subarray(Number(p), Number(p + n)));
    return n;
  },
  open(p, flags, _mode) {
    if (cstr(p) === "/proc/self/cmdline" && (flags & 3) === 0) {
      const fd = nextFd++;
      files.set(fd, { data: cmdline, pos: 0 });
      return fd;
    }
    return -1;
  },
  read(fd, p, n) {
    const f = files.get(fd);
    if (!f) return -1n;
    const k = Math.min(Number(n), f.data.length - f.pos);
    bytes().set(f.data.subarray(f.pos, f.pos + k), Number(p));
    f.pos += k;
    return BigInt(k);
  },
  close(fd) {
    return files.delete(fd) ? 0 : -1;
  },
  exit(code) {
    throw new Exit(code);
  },
  _exit(code) {
    throw new Exit(code);
  },
  abort() {
    throw new Exit(134);
  },
  // (C stdio's buffer, as glibc keeps it for a stdout that is not a terminal: written at fflush and at exit)
  putchar(c) {
    stdio.push(c & 255);
    return c;
  },
  sigaction: () => 0,
  fflush() {
    flushStdio();
    return 0;
  },
  getenv: () => 0n,
  strtod,
  strtof: (s, end) => Math.fround(strtod(s, end)),
  memmem(h, hl, n, nl) {
    const b = bytes();
    const hay = b.subarray(Number(h), Number(h + hl));
    const k = Buffer.from(hay).indexOf(Buffer.from(b.subarray(Number(n), Number(n + nl))));
    return k < 0 ? 0n : h + BigInt(k);
  },
  ...math,
};

const missing = WebAssembly.Module.imports(mod).filter((i) => i.module !== "env" || !(i.name in env));
if (missing.length > 0) {
  console.error("sandbox: the module imports what this host does not give: " + missing.map((i) => `${i.module}.${i.name}`).join(", "));
  process.exit(126);
}
const inst = new WebAssembly.Instance(mod, { env });
memory = inst.exports.memory;
let code;
try {
  code = inst.exports.main();
} catch (e) {
  if (e instanceof Exit) {
    code = e.code;
  } else if (e instanceof WebAssembly.RuntimeError) {
    const m = String(e.message);
    console.error(`wasm trap: ${m}`);
    code = /unreachable/.test(m) ? 101 : /memory access out of bounds/.test(m) ? 139 : 134;
  } else if (e instanceof RangeError) {
    console.error(`wasm: ${e.message}`);
    code = 134;
  } else {
    throw e;
  }
}
flushStdio();
process.exit(code);
