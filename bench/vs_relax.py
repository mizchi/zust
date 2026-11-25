"""zustc / rustc / relax (makepad/relax、JIT で動かす Rust の実装) を、ソースから出力が出終わるまでの時間で比べる。
usage: RELAX=<relax の checkout>/target/release/relax python3 bench/vs_relax.py [NAME ...]
  (このディレクトリで実行。事前に cargo build --release。NAME は gen1000 か bench/rt/NAME.rs。省略するとすべて)

relax はビルドしてから実行するのではなく、実行しながら関数ごとにコンパイルする。だからビルドの時間だけを比べられない。
ここでは 3 つとも「ソースを渡してから、プログラムが出力を出し終わるまで」を測る (時間は 3 回の中央値):
  zustc   zustc でビルドし、出来たものを実行する
  rustc   rustc -Cincremental (debug) でビルドし、実行する
  relax   relax run --quiet (パッケージの target/relax にキャッシュを持つ)
場面: cold (キャッシュなし) / no-op (何も変えずにもう一度) / edit (1 か所変えてもう一度。編集は vs_rustc.py と同じ)
cache はキャッシュのディスク上の大きさ (zustc: キャッシュ + .o、rustc: incremental ディレクトリ、relax: target/relax)。
出力は rustc -O の出力と一致するか確かめる"""
import os, shutil, statistics, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from vs_rustc import du, source  # noqa: E402

ZUSTC = os.path.abspath("target/release/zustc")
RELAX = os.environ.get("RELAX") or shutil.which("relax") or "/home/user/relax/target/release/relax"
W = os.path.abspath("out/vs_relax")


def run(cmd, cwd=None):
    t = time.perf_counter()
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=cwd)
    if r.returncode != 0:
        raise RuntimeError(f"failed: {' '.join(cmd)}\n{r.stderr.decode()[:2000]}")
    return (time.perf_counter() - t) * 1e3, r.stdout


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

    def scenario(label, write, go, store):
        def cold():
            for p in store:
                shutil.rmtree(p, ignore_errors=True) if os.path.isdir(p) else (os.path.exists(p) and os.remove(p))
            write(0)
            return go()[0]

        try:
            c = median3(cold)
            noop = median3(lambda: go()[0])
            k = [0]

            def edit():
                k[0] += 1
                write(k[0])
                return go()[0]

            e = median3(edit)
            write(0)
            out = go()[1]
            rows.append((label, c, noop, e, du(*store), out == want))
        except RuntimeError as ex:
            print(f"{name} / {label}: {str(ex).splitlines()[1] if len(str(ex).splitlines()) > 1 else ex}", file=sys.stderr)
            rows.append((label, None, None, None, None, None))

    def write_file(path):
        def w(v):
            with open(path, "w") as f:
                f.write(source(name, v))
        return w

    def build_run(build, exe):
        def go():
            t = run(build)[0]
            t2, out = run([exe])
            return t + t2, out
        return go

    exe = os.path.join(d, "z")
    scenario("zustc", write_file(src), build_run([ZUSTC, src, "-o", exe], exe), [exe + ".cache", exe + ".o"])
    exe, inc = os.path.join(d, "r"), os.path.join(d, "r-incr")
    scenario("rustc -Cincremental", write_file(src),
             build_run(["rustc", "--edition", "2021", "-Awarnings", "-Cincremental=" + inc, src, "-o", exe], exe), [inc])
    # relax: Cargo パッケージにする (名前で渡す)
    pkg = "p_" + name.replace("-", "_")
    pd = os.path.join(d, pkg)
    os.makedirs(os.path.join(pd, "src"))
    with open(os.path.join(pd, "Cargo.toml"), "w") as f:
        f.write(f'[package]\nname = "{pkg}"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n')
    scenario("relax run", write_file(os.path.join(pd, "src/main.rs")),
             lambda: run([RELAX, "run", "--quiet", pkg], cwd=pd), [os.path.join(pd, "target")])

    f = lambda x: "-" if x is None else f"{x:.0f}"
    print(f"\n### {name}\n")
    print("| 設定 | cold ms | no-op ms | edit ms | cache KiB | 出力 |")
    print("|---|---:|---:|---:|---:|---|")
    for label, c, noop, e, disk, same in rows:
        dk = "-" if disk is None else f"{disk / 1024:.1f}"
        sm = "-" if same is None else ("一致" if same else "不一致")
        print(f"| {label} | {f(c)} | {f(noop)} | {f(e)} | {dk} | {sm} |")
    sys.stdout.flush()


if __name__ == "__main__":
    print(f"relax: {RELAX} / {subprocess.run(['rustc', '--version'], capture_output=True, text=True).stdout.strip()}")
    names = sys.argv[1:] or ["gen1000"] + sorted(f[:-3] for f in os.listdir("bench/rt") if f.endswith(".rs"))
    for n in names:
        bench(n)
