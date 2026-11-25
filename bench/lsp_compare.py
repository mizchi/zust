#!/usr/bin/env python3
"""A comparison of LSP servers: runs zust-analyzer / rust-analyzer / rust-glancer through the same steps and measures them.

usage: python3 bench/lsp_compare.py [--servers zustc,rust-analyzer,rust-glancer] [--targets synth,zustc]
                                    [--out bench/lsp_compare_result.md] [--reps 10] [--timeout 300]
(run in this directory; cargo build --release --workspace beforehand (builds target/release/zust-analyzer); rust-glancer comes from RUST_GLANCER=<binary> or PATH)

What is measured (per server / per target):
- init: the time until the response to initialize
- ready: the time from startup until workspace/symbol returns a known symbol of the target (until the index is ready). Queries every 0.1 s
- docsym: the response time of textDocument/documentSymbol (median) and the number of symbols (counted with the tree flattened)
- wsym: the response time of workspace/symbol (median over several queries) and whether the known names were found
- edit→docsym: the time of documentSymbol right after a didChange (adding one fn), and whether the added fn appeared
- edit→diag: from didChange until publishDiagnostics (for that document) arrives (- if it never does)
- peak RSS: the server process's VmHWM, and the peak RSS summed with its descendant processes (cargo metadata, proc-macro-srv, etc.) (sampled every 50 ms)

Requests from the server (workspace/configuration, window/workDoneProgress/create, client/registerCapability, etc.) are answered with empty results.
Targets:
- synth: a generated Cargo crate (40 modules, each with 25 fns + struct / enum / impl)
- zustc: this directory (a Cargo workspace; its dependencies include cranelift etc.)
"""

import argparse, json, os, shutil, statistics, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def uri(p):
    return "file://" + os.path.abspath(p)


class Lsp:
    def __init__(self, cmd, root, env=None):
        self.p = subprocess.Popen(cmd, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
        self.next_id = 1
        self.lock = threading.Lock()
        self.waiting = {}
        self.diags = []  # (time, uri, version)
        self.alive = True
        self.tree_peak = 0
        # (the progress of the server's work: the tokens of unfinished `$/progress`, and rust-analyzer's experimental/serverStatus)
        self.progress = set()
        self.quiescent = None
        self.last_progress = time.time()
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._sample, daemon=True).start()

    def _sample(self):
        """The peak of the total RSS of the server and its descendant processes (every 50 ms)"""
        while self.p.poll() is None:
            kids = {}
            for d in os.listdir("/proc"):
                if d.isdigit():
                    try:
                        st = open(f"/proc/{d}/stat").read()
                        kids.setdefault(int(st[st.rindex(")") + 2:].split()[1]), []).append(int(d))
                    except (OSError, ValueError):
                        pass
            tot, todo = 0, [self.p.pid]
            while todo:
                q = todo.pop()
                try:
                    for l in open(f"/proc/{q}/status"):
                        if l.startswith("VmRSS:"):
                            tot += int(l.split()[1])
                except OSError:
                    pass
                todo += kids.get(q, [])
            self.tree_peak = max(self.tree_peak, tot)
            time.sleep(0.05)

    def _send(self, msg):
        b = json.dumps(msg).encode()
        try:
            self.p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
            self.p.stdin.flush()
        except (BrokenPipeError, OSError):
            self.alive = False

    def _read(self):
        f = self.p.stdout
        while True:
            n = None
            while True:
                line = f.readline()
                if not line:
                    self.alive = False
                    for ev in self.waiting.values():
                        ev[0].set()
                    return
                line = line.strip()
                if not line:
                    break
                if line.lower().startswith(b"content-length:"):
                    n = int(line.split(b":")[1])
            body = f.read(n)
            try:
                m = json.loads(body)
            except Exception:
                continue
            if "id" in m and "method" in m:
                # A request from the server: answer with an empty result
                meth = m["method"]
                if meth == "workspace/configuration":
                    res = [None for _ in m.get("params", {}).get("items", [])]
                else:
                    res = None
                self._send({"jsonrpc": "2.0", "id": m["id"], "result": res})
            elif "id" in m:
                with self.lock:
                    ev = self.waiting.get(m["id"])
                if ev:
                    ev[1].append(m)
                    ev[0].set()
            elif m.get("method") == "$/progress":
                pr = m.get("params", {})
                kind = (pr.get("value") or {}).get("kind")
                if kind == "begin":
                    self.progress.add(str(pr.get("token")))
                elif kind == "end":
                    self.progress.discard(str(pr.get("token")))
                self.last_progress = time.time()
            elif m.get("method") == "experimental/serverStatus":
                self.quiescent = bool((m.get("params") or {}).get("quiescent"))
                self.last_progress = time.time()
            elif m.get("method") == "textDocument/publishDiagnostics":
                pr = m.get("params", {})
                self.diags.append((time.time(), pr.get("uri"), pr.get("version")))

    def request(self, method, params, timeout=60):
        with self.lock:
            i = self.next_id
            self.next_id += 1
            ev = (threading.Event(), [])
            self.waiting[i] = ev
        self._send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
        ok = ev[0].wait(timeout)
        with self.lock:
            self.waiting.pop(i, None)
        if not ok or not ev[1]:
            return None
        return ev[1][0]

    def wait_idle(self, timeout, quiet=0.5):
        """Waits until the server's work (`$/progress`, serverStatus) is finished and nothing has arrived for quiet seconds"""
        t0 = time.time()
        while time.time() - t0 < timeout and self.alive:
            busy = self.progress or self.quiescent is False
            if not busy and time.time() - self.last_progress >= quiet:
                return True
            time.sleep(0.05)
        return False

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def peak_rss_mb(self):
        try:
            for l in open(f"/proc/{self.p.pid}/status"):
                if l.startswith("VmHWM:"):
                    return int(l.split()[1]) / 1024
        except OSError:
            pass
        return None

    def close(self):
        if self.alive:
            self.request("shutdown", None, timeout=5)
            self.notify("exit", None)
        try:
            self.p.wait(5)
        except subprocess.TimeoutExpired:
            self.p.kill()


def flat_count(syms):
    n = 0
    for s in syms or []:
        n += 1
        n += flat_count(s.get("children"))
    return n


def flat_names(syms):
    out = []
    for s in syms or []:
        out.append(s.get("name"))
        out += flat_names(s.get("children"))
    return out


# ---------------------------------------------------------------- targets


def make_synth(dir, mods=40):
    """A generated Cargo crate: src/main.rs + src/m{k}.rs (mods of them; default 40, 1000 for big)"""
    os.makedirs(os.path.join(dir, "src"), exist_ok=True)
    open(os.path.join(dir, "Cargo.toml"), "w").write('[package]\nname = "synth"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n')
    for k in range(mods):
        lines = [f"//! module {k}", "", f"pub struct Point{k} {{ pub x: i64, pub y: i64 }}", "",
                 f"pub enum Shape{k} {{ Circle(i64), Rect {{ w: i64, h: i64 }}, Empty }}", "",
                 f"impl Point{k} {{", f"    pub fn new(x: i64, y: i64) -> Point{k} {{ Point{k} {{ x, y }} }}",
                 f"    pub fn norm{k}(&self) -> i64 {{ self.x * self.x + self.y * self.y }}", "}", ""]
        for j in range(25):
            prev = f"f{k}_{j - 1}(n + 1)" if j > 0 else "n"
            lines += [f"pub fn f{k}_{j}(n: i64) -> i64 {{", f"    let p = Point{k}::new(n, {j});", f"    p.norm{k}() + {prev}", "}", ""]
        open(os.path.join(dir, "src", f"m{k}.rs"), "w").write("\n".join(lines))
    main = [f"mod m{k};" for k in range(mods)] + ["", "fn main() {", "    let mut s = 0;"]
    main += [f"    s += m{k}::f{k}_0({k});" for k in range(mods)]
    main += ['    println!("{s}");', "}", ""]
    open(os.path.join(dir, "src", "main.rs"), "w").write("\n".join(main))
    return {
        "root": dir,
        "open": "src/m7.rs",
        "ready": ("norm31", "norm31"),
        "queries": ["norm1", "Point2", "f39_24", "Shape", "rect"],
        "expect_doc": "norm7",
        "edit_after": "pub enum Shape7",
        "refs": "Point7",
    }


def zustc_target():
    # (ZUST_LSP_ROOT: another copy of the sources, e.g. a fixed snapshot, so that the target does not change with the code)
    return {
        "root": os.environ.get("ZUST_LSP_ROOT") or HERE,
        "open": "crates/core/src/driver2.rs",
        "ready": ("build_workspace", "build_workspace"),
        "queries": ["Session2", "check", "split_inc", "Recorder", "lower"],
        "expect_doc": "check",
        "edit_after": "pub struct Diagnostic",
        "refs": "Session2",
    }


# ---------------------------------------------------------------- measuring


def bench(name, cmd, t, reps, timeout, env=None):
    r = {"server": name}
    root = t["root"]
    path = os.path.join(root, t["open"])
    text = open(path).read()
    u = uri(path)
    t0 = time.time()
    s = Lsp(cmd, root, env)
    caps = {
        "textDocument": {"documentSymbol": {"hierarchicalDocumentSymbolSupport": True}, "publishDiagnostics": {"versionSupport": True},
                         "synchronization": {"didSave": True}},
        "workspace": {"symbol": {}, "configuration": True, "workspaceFolders": True},
        "window": {"workDoneProgress": True},
    }
    init = s.request("initialize", {"processId": os.getpid(), "rootUri": uri(root), "rootPath": root, "capabilities": caps,
                                    "workspaceFolders": [{"uri": uri(root), "name": os.path.basename(root)}]}, timeout=timeout)
    r["init_ms"] = (time.time() - t0) * 1000 if init else None
    if not init:
        s.close()
        r["error"] = "no initialize response"
        return r
    s.notify("initialized", {})
    s.notify("textDocument/didOpen", {"textDocument": {"uri": u, "languageId": "rust", "version": 1, "text": text}})
    # Until the index is ready: until a known name shows up in workspace/symbol
    q, want = t["ready"]
    r["ready_ms"] = None
    while time.time() - t0 < timeout and s.alive:
        m = s.request("workspace/symbol", {"query": q}, timeout=timeout)
        res = (m or {}).get("result") or []
        if any(x.get("name") == want for x in res):
            r["ready_ms"] = (time.time() - t0) * 1000
            break
        time.sleep(0.1)
    # documentSymbol
    ts = []
    n = None
    names = []
    for _ in range(reps):
        a = time.time()
        m = s.request("textDocument/documentSymbol", {"textDocument": {"uri": u}}, timeout=60)
        ts.append((time.time() - a) * 1000)
        res = (m or {}).get("result")
        if res is not None:
            n = flat_count(res)
            names = flat_names(res)
    r["docsym_ms"] = statistics.median(ts)
    r["docsym_n"] = n
    r["docsym_ok"] = t["expect_doc"] in names
    # workspace/symbol
    ts = []
    found = 0
    for q in t["queries"]:
        for _ in range(max(1, reps // 2)):
            a = time.time()
            m = s.request("workspace/symbol", {"query": q}, timeout=60)
            ts.append((time.time() - a) * 1000)
        res = (m or {}).get("result") or []
        if any(q.lower() in (x.get("name") or "").lower() for x in res):
            found += 1
    r["wsym_ms"] = statistics.median(ts)
    r["wsym_found"] = f"{found}/{len(t['queries'])}"
    # Edit: add one fn
    at = text.find(t["edit_after"])
    at = text.rfind("\n", 0, at) + 1 if at >= 0 else len(text)
    new = text[:at] + "pub fn added_by_bench() -> u32 { 1 }\n\n" + text[at:]
    nd = len(s.diags)
    a = time.time()
    s.notify("textDocument/didChange", {"textDocument": {"uri": u, "version": 2}, "contentChanges": [{"text": new}]})
    m = s.request("textDocument/documentSymbol", {"textDocument": {"uri": u}}, timeout=60)
    r["edit_docsym_ms"] = (time.time() - a) * 1000
    r["edit_docsym_ok"] = "added_by_bench" in flat_names((m or {}).get("result"))
    r["edit_diag_ms"] = None
    while time.time() - a < 20:
        hit = [d for d in s.diags[nd:] if d[1] == u]
        if hit:
            r["edit_diag_ms"] = (hit[0][0] - a) * 1000
            break
        time.sleep(0.01)
    # References (searches the whole workspace): at the first position of a name in the open file. The first and the second time
    r["refs_ms"], r["refs_n"] = None, None
    if t.get("refs") and "referencesProvider" in str((init.get("result") or {}).get("capabilities", {})):
        i = new.find(t["refs"])
        li = new.count("\n", 0, i)
        col = i - (new.rfind("\n", 0, i) + 1)
        ts = []
        for _ in range(2):
            a = time.time()
            m = s.request("textDocument/references", {"textDocument": {"uri": u}, "position": {"line": li, "character": col + 1},
                                                      "context": {"includeDeclaration": True}}, timeout=timeout)
            ts.append((time.time() - a) * 1000)
            res = (m or {}).get("result")
            r["refs_n"] = len(res) if isinstance(res, list) else None
        r["refs_ms"] = ts
    r["peak_rss_mb"] = s.peak_rss_mb()
    r["tree_rss_mb"] = s.tree_peak / 1024
    s.close()
    return r


def fmt(x, unit=""):
    if x is None:
        return "-"
    if isinstance(x, bool):
        return "yes" if x else "no"
    if isinstance(x, float):
        return f"{x:.1f}{unit}" if x < 100 else f"{x:.0f}{unit}"
    return f"{x}{unit}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--servers", default="zustc,rust-analyzer,rust-glancer")
    ap.add_argument("--targets", default="synth,zustc,big")
    ap.add_argument("--reps", type=int, default=10)
    ap.add_argument("--timeout", type=float, default=300)
    ap.add_argument("--out", default=None)
    ap.add_argument("--cold", action="store_true", help="rust-glancer の索引 (target/rust_glancer) を消してから測る")
    a = ap.parse_args()
    glancer = os.environ.get("RUST_GLANCER") or shutil.which("rust-glancer") or "/home/user/rust-glancer/target/release/rust-glancer"
    servers = {
        "zustc": [os.path.join(HERE, "target/release/zust-analyzer")],
        "rust-analyzer": [os.environ.get("RUST_ANALYZER") or "rust-analyzer"],
        "rust-glancer": [glancer, "lsp"],
    }
    tmp = tempfile.mkdtemp(prefix="lsp_compare_")
    targets = {}
    for t in a.targets.split(","):
        if t == "synth":
            targets["synth"] = make_synth(os.path.join(tmp, "synth"))
        elif t == "big":
            targets["big"] = make_synth(os.path.join(tmp, "big"), 1000)
        elif t == "zustc":
            targets["zustc"] = zustc_target()
    out = ["| 対象 | サーバ | init | ready | docsym (数) | wsym (見つかった) | edit→docsym | edit→diag | references 1 回目 / 2 回目 (数) | peak RSS (子も) |",
           "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for tn, t in targets.items():
        for sn in a.servers.split(","):
            cmd = servers[sn]
            if not (os.path.exists(cmd[0]) or shutil.which(cmd[0])):
                print(f"skip {sn}: {cmd[0]} not found", file=sys.stderr)
                continue
            if a.cold and sn == "rust-glancer":
                shutil.rmtree(os.path.join(t["root"], "target", "rust_glancer"), ignore_errors=True)
            print(f"== {tn} / {sn}", file=sys.stderr)
            r = bench(sn, cmd, t, a.reps, a.timeout)
            print(json.dumps(r), file=sys.stderr)
            if r.get("error"):
                out.append(f"| {tn} | {sn} | {r['error']} | | | | | | |")
                continue
            ds = f"{fmt(r['docsym_ms'], ' ms')} ({fmt(r['docsym_n'])}{'' if r['docsym_ok'] else ', 期待の名前なし'})"
            ed = f"{fmt(r['edit_docsym_ms'], ' ms')}{'' if r['edit_docsym_ok'] else ' (足した関数なし)'}"
            rf = "-" if not r.get("refs_ms") else f"{fmt(r['refs_ms'][0], ' ms')} / {fmt(r['refs_ms'][1], ' ms')} ({fmt(r['refs_n'])})"
            out.append(f"| {tn} | {sn} | {fmt(r['init_ms'], ' ms')} | {fmt(r['ready_ms'], ' ms')} | {ds} | {fmt(r['wsym_ms'], ' ms')} ({r['wsym_found']}) | "
                       f"{ed} | {fmt(r['edit_diag_ms'], ' ms')} | {rf} | {fmt(r['peak_rss_mb'], ' MB')} ({fmt(r['tree_rss_mb'], ' MB')}) |")
    print("\n".join(out))
    if a.out:
        open(a.out, "w").write("\n".join(out) + "\n")
    shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
