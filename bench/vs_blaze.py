"""zustc / rustc / blaze (indexable-inc/blaze, a cargo replacement whose daemon keeps the workspace compiled), on the
same programs on this machine: build times, what each keeps on disk and in memory.

usage: python3 bench/vs_blaze.py [NAME ...]
  (run from the repository root after cargo build --release; NAME is gen1000 or bench/rt/NAME.rs, all if none;
   blaze from the PATH or $BLAZE)

Each program is built three ways (times are medians of 3 runs, the program's output is checked against rustc -O):
  zustc   zustc p.rs -o p (its cache next to the output)
  rustc   rustc -Cincremental (debug)
  blaze   blaze build in a cargo package holding the program (dev profile; its daemon stays up between builds, as it is
          meant to be used)
Scenes: cold (no cache; for blaze also no daemon: `blaze stop` first), no-op (build again, nothing changed), edit
(change one place and build again; the edits are vs_rustc.py's).
disk: zustc's cache and object file, rustc's incremental directory, blaze's target directory.
memory: the peak RSS of one edit build for zustc and rustc; for blaze, the RSS its daemon processes hold after the
edits (they stay resident).
"""
import os, resource, shutil, statistics, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vs_rustc import du, source  # noqa: E402

ZUSTC = os.environ.get("ZUSTC") or os.path.abspath("target/release/zustc")
BLAZE = os.environ.get("BLAZE") or shutil.which("blaze") or os.path.expanduser("~/.cargo/bin/blaze")
W = os.path.abspath("out/vs_blaze")
# (rustc overflows its stack on gen5000's main)
os.environ.setdefault("RUST_MIN_STACK", str(256 << 20))


def run(cmd, cwd=None):
    t = time.perf_counter()
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=cwd)
    if r.returncode != 0:
        raise RuntimeError(f"failed: {' '.join(cmd)}\n{r.stderr.decode()[-2000:]}")
    return (time.perf_counter() - t) * 1e3, r.stdout


def peak_rss_mib(cmd, cwd=None):
    """peak RSS of one run of cmd (through a child Python, so RUSAGE_CHILDREN is that run's alone)"""
    code = ("import resource,subprocess,sys;subprocess.run(sys.argv[1:],check=True,stdout=subprocess.DEVNULL,"
            "stderr=subprocess.DEVNULL);print(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)")
    out = subprocess.run([sys.executable, "-c", code, *cmd], capture_output=True, text=True, cwd=cwd, check=True).stdout
    return int(out) / 1024


def daemon_rss_mib():
    """the RSS of blaze's daemon processes (ra-jit), summed"""
    out = subprocess.run(["ps", "-eo", "rss,comm"], capture_output=True, text=True).stdout
    return sum(int(l.split()[0]) for l in out.splitlines()[1:] if "ra-jit" in l or "blaze" in l.split()[-1]) / 1024


def median3(f):
    return statistics.median(f() for _ in range(3))


def bench(name):
    d = os.path.join(W, name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src = os.path.join(d, "p.rs")
    with open(src, "w") as f:
        f.write(source(name, 0))
    ref = os.path.join(d, "ref")
    run(["rustc", "--edition", "2021", "-Awarnings", "-O", src, "-o", ref])
    want = run([ref])[1]
    rows = []

    def scenario(label, write, build, exe, store, before_cold=None, cwd=None, mem=None):
        try:
            def cold():
                if before_cold:
                    before_cold()
                for p in store:
                    shutil.rmtree(p, ignore_errors=True) if os.path.isdir(p) else (os.path.exists(p) and os.remove(p))
                write(0)
                return run(build, cwd)[0]

            c = median3(cold)
            noop = median3(lambda: run(build, cwd)[0])
            k = [0]

            def edit():
                k[0] += 1
                write(k[0])
                return run(build, cwd)[0]

            e = median3(edit)
            write(0)
            run(build, cwd)
            out = run([exe])[1]
            if not mem:
                # (an edit build's: the build before it left the output fresh, and a no-op build does nothing)
                write(k[0] + 1)
            m = mem() if mem else peak_rss_mib(build, cwd)
            rows.append((label, c, noop, e, du(*store), m, out == want))
        except RuntimeError as ex:
            print(f"{name} / {label}: {str(ex)[:300]}", file=sys.stderr)
            rows.append((label, None, None, None, None, None, None))

    def write_file(path):
        def w(v):
            with open(path, "w") as f:
                f.write(source(name, v))
        return w

    exe = os.path.join(d, "z")
    scenario("zustc", write_file(src), [ZUSTC, src, "-o", exe], exe, [exe + ".crcache2", exe + ".o"])
    exe, inc = os.path.join(d, "r"), os.path.join(d, "r-incr")
    scenario("rustc -Cincremental", write_file(src),
             ["rustc", "--edition", "2021", "-Awarnings", "-Cincremental=" + inc, src, "-o", exe], exe, [inc])
    pkg = "p_" + name.replace("-", "_")
    pd = os.path.join(d, pkg)
    os.makedirs(os.path.join(pd, "src"))
    with open(os.path.join(pd, "Cargo.toml"), "w") as f:
        f.write(f'[package]\nname = "{pkg}"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n')
    target = os.path.join(pd, "target")
    scenario("blaze build", write_file(os.path.join(pd, "src/main.rs")), [BLAZE, "build", "-q"],
             os.path.join(target, "debug", pkg), [target], before_cold=lambda: subprocess.run([BLAZE, "stop"], capture_output=True),
             cwd=pd, mem=daemon_rss_mib)

    f = lambda x: "-" if x is None else f"{x:.0f}"
    print(f"\n### {name}\n")
    print("| build | cold ms | no-op ms | edit ms | disk KiB | memory MiB | output |")
    print("|---|---:|---:|---:|---:|---:|---|")
    for label, c, noop, e, disk, m, same in rows:
        dk = "-" if disk is None else f"{disk / 1024:.0f}"
        mm = "-" if m is None else f"{m:.0f}"
        sm = "-" if same is None else ("same" if same else "differs")
        print(f"| {label} | {f(c)} | {f(noop)} | {f(e)} | {dk} | {mm} | {sm} |")
    sys.stdout.flush()


if __name__ == "__main__":
    ver = subprocess.run([BLAZE, "--version"], capture_output=True, text=True).stdout.strip()
    print(f"blaze: {ver} / {subprocess.run(['rustc', '--version'], capture_output=True, text=True).stdout.strip()} / {os.cpu_count()} cores")
    names = sys.argv[1:] or ["gen1000"] + sorted(f[:-3] for f in os.listdir("bench/rt") if f.endswith(".rs"))
    for n in names:
        bench(n)
