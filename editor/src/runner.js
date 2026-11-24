// Runs inside the preview iframe (a module script of its srcdoc, as text: Vite does not touch it).
// It gets the compiled module by postMessage, imports it, calls `main` and renders a React element it returns.
const post = (kind, text) => parent.postMessage({ zsc: true, kind, text }, "*");

// (`path`: the arrays and objects being shown, to mark a cycle; a value seen twice side by side is shown twice)
const show = (v, top = true, path = []) => {
  if (typeof v === "string") return top ? v : JSON.stringify(v);
  if (typeof v === "bigint") return `${v}`;
  if (typeof v === "function") return `[function ${v.name || "anonymous"}]`;
  if (v === undefined) return "()";
  if (v && typeof v === "object") {
    if (path.includes(v)) return "[circular]";
    if ("$$typeof" in v) return "<React element>";
    path.push(v);
    try {
      if (Array.isArray(v)) return `[${v.map((x) => show(x, false, path)).join(", ")}]`;
      return `{ ${Object.entries(v).map(([k, x]) => `${k}: ${show(x, false, path)}`).join(", ")} }`;
    } finally {
      path.pop();
    }
  }
  return String(v);
};

for (const k of ["log", "info", "warn", "error"]) {
  const orig = console[k].bind(console);
  console[k] = (...a) => {
    orig(...a);
    post(k, a.map((x) => show(x)).join(" "));
  };
}

const fail = (e) => {
  if (e instanceof Error) post("error", e.stack && e.stack.includes(e.message) ? e.stack : `${e.name}: ${e.message}`);
  else post("error", String(e));
};
addEventListener("error", (e) => fail(e.error ?? e.message));
addEventListener("unhandledrejection", (e) => fail(e.reason));

addEventListener("message", async (e) => {
  const code = e.data && e.data.code;
  if (typeof code !== "string") return;
  const url = URL.createObjectURL(new Blob([code], { type: "text/javascript" }));
  try {
    const m = await import(url);
    if (typeof m.main !== "function") {
      const names = Object.keys(m).filter((n) => typeof m[n] === "function");
      post("system", `no \`export fn main\` to run${names.length ? ` (exports: ${names.join(", ")})` : ""}`);
      return;
    }
    const t0 = performance.now();
    let r = m.main();
    if (r && typeof r.then === "function") r = await r;
    const ms = (performance.now() - t0).toFixed(1);
    if (r && typeof r === "object" && "$$typeof" in r) {
      post("render", "");
      const { createRoot } = await import("react-dom/client");
      createRoot(document.getElementById("root"), { onUncaughtError: fail, onCaughtError: fail }).render(r);
      post("system", `main() returned a React element: rendered above (${ms} ms)`);
    } else {
      if (r !== undefined) post("result", show(r));
      post("system", `main() returned in ${ms} ms`);
    }
  } catch (err) {
    fail(err);
  } finally {
    URL.revokeObjectURL(url);
    post("done", "");
  }
});

post("ready", "");
