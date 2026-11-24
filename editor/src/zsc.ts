// The compiler: zsc (crates/zustscript) built to wasm by zsc-wasm/ (`npm run build:zsc`)

export type Diagnostic = { severity: "error" | "warning"; line: number; col: number; message: string };
export type Output = { ok: boolean; js: string; dts: string; diagnostics: Diagnostic[]; ms: number };

type Exports = {
  memory: WebAssembly.Memory;
  zsc_alloc(n: number): number;
  zsc_free(p: number, n: number): void;
  zsc_compile(p: number, n: number): number;
  zsc_result(): number;
};

export async function loadCompiler(url: string): Promise<(src: string) => Output> {
  const { instance } = await WebAssembly.instantiateStreaming(fetch(url), {});
  const x = instance.exports as unknown as Exports;
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  return (src) => {
    const t0 = performance.now();
    const b = enc.encode(src);
    const p = x.zsc_alloc(b.length);
    new Uint8Array(x.memory.buffer, p, b.length).set(b);
    let n: number;
    try {
      n = x.zsc_compile(p, b.length);
    } catch (e) {
      // (a panic aborts: the instance is unusable afterwards, so the page reloads it on the next edit)
      throw new CompilerCrash(String(e));
    } finally {
      try { x.zsc_free(p, b.length); } catch { /* the instance trapped */ }
    }
    const r = JSON.parse(dec.decode(new Uint8Array(x.memory.buffer, x.zsc_result(), n))) as Omit<Output, "ms">;
    return { ...r, ms: performance.now() - t0 };
  };
}

export class CompilerCrash extends Error {}
