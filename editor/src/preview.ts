import runner from "./runner.js?raw";

// The preview: the compiled module runs in a sandboxed iframe (an opaque origin, a new one per run).
// `export fn main` is called; when it returns a React element it is rendered (React from a CDN, through an import map).

export type LogKind = "log" | "info" | "warn" | "error" | "result" | "system";
/** what the run did besides logging: `render` when `main` returned a React element */
export type RunEvent = "render" | "done";
export type LogLine = { kind: LogKind; text: string };

const REACT = "19.1.0";
export const CDN = "https://esm.sh";

const IMPORT_MAP = {
  imports: {
    react: `${CDN}/react@${REACT}`,
    "react/jsx-runtime": `${CDN}/react@${REACT}/jsx-runtime`,
    "react-dom/client": `${CDN}/react-dom@${REACT}/client?deps=react@${REACT}`,
  },
};

/** Another bare import (`extern "pkg" fn ..`) is loaded from the CDN too */
export function rewriteImports(js: string): string {
  return js.replace(/^(import\b[^;\n]*?\bfrom\s*)"([^"]+)"/gm, (all, head: string, spec: string) => {
    if (spec in IMPORT_MAP.imports || /^(\.|\/|[a-z]+:)/.test(spec)) return all;
    return `${head}"${CDN}/${spec}"`;
  });
}

const STYLE = `
  :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
  body { margin: 12px; background: Canvas; color: CanvasText; }
  button { font: inherit; padding: 4px 10px; }
  li { cursor: default; }
`;

function srcdoc(): string {
  return `<!doctype html><html><head><meta charset="utf-8">
<script type="importmap">${JSON.stringify(IMPORT_MAP)}</script>
<style>${STYLE}</style>
<script type="module">${runner.replaceAll("</script", "<\\/script")}</script>
</head><body><div id="root"></div></body></html>`;
}

/** Runs `js` in a new iframe in `host`; `onLog` gets its console output, `onEvent` a render and the end of the run */
export function run(host: HTMLElement, js: string, onLog: (l: LogLine) => void, onEvent: (e: RunEvent) => void): void {
  host.replaceChildren();
  const frame = document.createElement("iframe");
  frame.sandbox.add("allow-scripts");
  frame.title = "preview";
  frame.srcdoc = srcdoc();
  const code = rewriteImports(js);
  const onMessage = (e: MessageEvent) => {
    if (e.source !== frame.contentWindow || !e.data?.zsc) return;
    const { kind, text } = e.data as { kind: string; text: string };
    if (kind === "ready") frame.contentWindow!.postMessage({ code }, "*");
    else if (kind === "done" || kind === "render") onEvent(kind);
    else onLog({ kind: kind as LogKind, text });
  };
  addEventListener("message", onMessage);
  // (the listener goes with the frame)
  new MutationObserver((_, obs) => {
    if (!frame.isConnected) {
      removeEventListener("message", onMessage);
      obs.disconnect();
    }
  }).observe(host, { childList: true });
  host.append(frame);
}
