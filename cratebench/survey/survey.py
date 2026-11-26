#!/usr/bin/env python3
"""How much of crates.io's most used code does zustc build? A survey of the top crates, in temporary directories.

usage (at the repository root, after `cargo build --release -p zustc -p zurgo`; needs the network for crates.io):
  python3 cratebench/survey/survey.py list [--top N]     refresh crates.tsv: the top N crates by recent downloads
  python3 cratebench/survey/survey.py lock               resolve Cargo.lock: one lock for every probe
  python3 cratebench/survey/survey.py run [NAME ..] [-j P] [--tests] [--work DIR] [--commit SHA] [--history]
                                                         survey, then write results.json and CRATES_SUPPORT.md
  python3 cratebench/survey/survey.py report [--commit SHA] [--history]
                                                         CRATES_SUPPORT.md (and a history.tsv row) from results.json

Nothing is vendored. For each crate of crates.tsv, `run` writes a probe package in the work directory (by default
$TMPDIR/zust-crate-survey; it is deleted and made again): a library whose one dependency is that crate, pinned with
`=`, with its default features, and the versions of Cargo.lock (the probes resolved together; cargo drops what a probe
does not use). cargo fetches the sources from crates.io. Then, for x86_64 Linux:
  1. `zurgo check`: the errors zustc reports in each crate of the probe's graph (a crate's errors do not stop the
     check of the others). This is a lower bound of what is left: zustc stops reading a crate at a syntax or macro
     error, and checks each function only up to its first error.
  2. `zurgo build`, when the check found nothing: the crate and its dependencies compiled and linked.
  3. with --tests, for the crates that build: the crate's own tests (lib, bins, tests/; not doc tests), copied from
     its crates.io package into the work directory, under `cargo test` and `zurgo test`, compared test by test.
A crate's status is the first of: graph-fail (zurgo cannot plan the graph), errors (its own code has errors),
blocked (its code is fine or unread, a dependency's is not), build-fail, builds. Proc-macro crates run on the host,
where rustc builds them, and crates for other platforms are listed but not probed.
"""
import argparse
import collections
import concurrent.futures
import datetime
import functools
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
LIST = os.path.join(HERE, "crates.tsv")
LOCK = os.path.join(HERE, "Cargo.lock")
RESULTS = os.path.join(HERE, "results.json")
HISTORY = os.path.join(HERE, "history.tsv")
DOC = os.path.join(ROOT, "CRATES_SUPPORT.md")
TODO = os.path.join(ROOT, "TODO.md")
ZURGO = os.environ.get("ZURGO") or os.path.join(os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target"), "release/zurgo")
TARGET = "x86_64-unknown-linux-gnu"
TIMEOUT = 900
UA = "zust-crate-survey (https://github.com/mizchi/zust)"
# (crates for another platform: built on Linux they are empty or fail as rustc's build would; listed, not probed)
OTHER_PLATFORM = re.compile(r"^(windows|winapi|core-foundation|security-framework|r-efi|wasi|wasip|redox_|wasm-bindgen|js-sys|"
                            r"web-sys|wit-bindgen|objc|android|ndk|hermit-abi|libredox|cocoa|io-kit)")
STATUSES = ["builds", "build-fail", "blocked", "errors", "graph-fail", "timeout", "fetch-fail", "survey-error"]


def sh(cmd, cwd=None, env=None, timeout=TIMEOUT):
    t = time.time()
    try:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env={**os.environ, **(env or {})}, timeout=timeout)
        return p.returncode, p.stdout, p.stderr, time.time() - t
    except subprocess.TimeoutExpired as e:
        out = e.stdout or ""
        if isinstance(out, bytes):
            out = out.decode("utf-8", "replace")
        return None, out, f"timeout ({timeout} s)", time.time() - t


def probe_id(name, features):
    """a probe's name: the crate's, with its features when they are not the defaults (`tokio[full]`)"""
    return f"{name}[{features}]" if features else name


def read_list():
    rows = []
    for line in open(LIST):
        if line.startswith("#") or not line.strip():
            continue
        rank, name, version, downloads, kind, category, features = (line.rstrip("\n").split("\t") + [""] * 7)[:7]
        rows.append(dict(id=probe_id(name, features), rank=int(rank), name=name, version=version, downloads=int(downloads),
                         kind=kind, category=category or "other", features=features))
    return rows


# (the survey's categories: crates of one kind tend to stop zustc in the same places)
CATEGORIES = {
    "helpers": "language helpers",
    "data-structures": "data structures and buffers",
    "text": "text, parsing, regex, Unicode",
    "serialization": "serialization, encodings, compression",
    "numeric": "numbers, math, random",
    "crypto-hash": "cryptography, hashing, checksums, TLS",
    "async": "async and concurrency",
    "network": "network and web",
    "system": "OS, FFI, files",
    "build-macro": "build scripts and proc-macro support",
    "cli-log-errors": "errors, logging, CLI, terminal",
    "i18n": "internationalization (ICU4X)",
    "time": "date and time",
    "embedded": "embedded and no_std",
    "other": "other",
}
# (crates.io's category slugs, first match wins)
SLUGS = [("embedded", "embedded"), ("development-tools::procedural-macro-helpers", "build-macro"),
         ("development-tools::build-utils", "build-macro"), ("internationalization", "i18n"), ("localization", "i18n"),
         ("date-and-time", "time"), ("cryptography", "crypto-hash"), ("authentication", "crypto-hash"),
         ("asynchronous", "async"), ("concurrency", "async"), ("network-programming", "network"),
         ("web-programming", "network"), ("encoding", "serialization"), ("value-formatting", "serialization"),
         ("parser-implementations", "text"), ("parsing", "text"), ("text-processing", "text"),
         ("data-structures", "data-structures"), ("memory-management", "data-structures"), ("mathematics", "numeric"),
         ("science", "numeric"), ("algorithms", "numeric"), ("os", "system"), ("filesystem", "system"),
         ("external-ffi-bindings", "system"), ("api-bindings", "system"), ("hardware-support", "system"),
         ("command-line-interface", "cli-log-errors"), ("command-line-utilities", "cli-log-errors"),
         ("development-tools", "cli-log-errors"), ("rust-patterns", "helpers"), ("no-std", "helpers")]
# (where crates.io's categories say little or mislead: by name)
BY_NAME = [
    (r"^(fnv|foldhash|ahash|rustc-hash|siphasher|crc32fast|crc|adler2?|simd-adler32|sha1|sha2|sha3|md-5|digest|block-buffer|"
     r"crypto-common|hmac|subtle|zeroize|const-oid|der|spki|pkcs8|signature|cipher|chacha20|ring|rustls.*|untrusted|"
     r"webpki-roots|tokio-rustls|hyper-rustls|aws-lc.*|blake3|xxhash-rust|twox-hash)$", "crypto-hash"),
    (r"^(getrandom|rand|rand_core|rand_chacha|ppv-lite86|fastrand|num-.*|libm|half|typenum|generic-array|hybrid-array)$", "numeric"),
    (r"^(anyhow|thiserror.*|log|tracing.*|displaydoc|env_logger|anstyle.*|anstream|colorchoice|is_terminal_polyfill|"
     r"nu-ansi-term|clap.*|strsim)$", "cli-log-errors"),
    (r"^(syn|quote|proc-macro2|unicode-ident|prettyplease|darling.*|synstructure|ident_case|proc-macro-crate|cc|autocfg|"
     r"version_check|pkg-config|shlex|rustc_version|cfg_aliases|find-msvc-tools|vcpkg|jobserver|glob)$", "build-macro"),
    (r"^(memchr|aho-corasick|regex.*|unicode-.*|utf8_iter|utf8parse|nom|winnow|minimal-lexical|encoding_rs|heck|"
     r"convert_case|matchers|bstr)$", "text"),
    (r"^(libc|rustix|linux-raw-sys|errno|nix|tempfile|walkdir|same-file|libloading|signal-hook.*|openssl-probe|memmap2|"
     r"object|cpufeatures)$", "system"),
    (r"^(icu_.*|zerovec.*|yoke.*|tinystr|writeable|litemap|zerotrie|potential_utf|zerofrom.*|idna_adapter)$", "i18n"),
    (r"^(idna|url|form_urlencoded|percent-encoding|httparse|httpdate|mime|ipnet|http.*|hyper.*|h2|tower.*|axum.*|"
     r"reqwest|quinn.*|socket2|matchit|serde_urlencoded)$", "network"),
    (r"^(toml.*|serde.*|schemars|prost.*|quick-xml|base64|base64ct|hex|data-encoding|itoa|ryu|zmij|miniz_oxide|zlib-rs|"
     r"flate2)$", "serialization"),
    (r"^itertools$", "helpers"),
    (r"^(bytes|smallvec|arrayvec|tinyvec|slab|indexmap|hashbrown|hashlink|bit-vec|bit-set|fixedbitset|petgraph|"
     r"sharded-slab|lru-slab|bumpalo|allocator-api2|phf.*)$", "data-structures"),
    (r"^(cfg-if|once_cell|lazy_static|either|scopeguard|static_assertions|pin-project.*|rustversion|equivalent|"
     r"stable_deref_trait|memoffset|bytemuck|zerocopy.*|ref-cast.*|dyn-clone|derive_more|strum.*|semver|uuid|"
     r"tinyvec_macros)$", "helpers"),
    (r"^(futures.*|mio|parking_lot.*|lock_api|crossbeam.*|rayon.*|event-listener|atomic-waker|want|try-lock|"
     r"sync_wrapper|thread_local|spin|portable-atomic|tokio.*)$", "async"),
    (r"^(chrono|time|time-.*|iana-time-zone|deranged|powerfmt|num-conv)$", "time"),
]


def category_of(name, slugs):
    for pat, cat in BY_NAME:
        if re.match(pat, name):
            return cat
    for s in slugs:
        for pre, cat in SLUGS:
            if s == pre or s.startswith(pre + "::"):
                return cat
    return "other"


# ---- list: the top crates by recent downloads ----

def api(path):
    req = urllib.request.Request("https://crates.io/api/v1/" + path, headers={"User-Agent": UA})
    data = json.load(urllib.request.urlopen(req, timeout=60))
    time.sleep(1)  # (crates.io's crawler policy: one request a second)
    return data


def cmd_list(a):
    # (the categories already in the list are kept: they may have been corrected by hand)
    known = {r["name"]: r["category"] for r in read_list()} if os.path.exists(LIST) else {}
    top, page = [], 1
    while len(top) < a.top:
        data = api(f"crates?sort=recent-downloads&per_page=100&page={page}")
        if not data["crates"]:
            break
        top += [c for c in data["crates"]][:a.top - len(top)]
        page += 1
    rows = []
    for c in top:
        name = c["name"]
        cat = known.get(name) or category_of(name, api(f"crates/{name}")["crate"].get("categories") or [])
        kind = "other-platform" if OTHER_PLATFORM.match(name) else "lib"
        rows.append([len(rows) + 1, name, c.get("max_stable_version") or c["newest_version"], c["recent_downloads"] or 0,
                     kind, cat, ""])
    for line in open(os.path.join(HERE, "extra.tsv")):
        if line.startswith("#") or not line.strip():
            continue
        name, features, cat, _why = line.rstrip("\n").split("\t")
        c = api(f"crates/{name}")["crate"]
        rows.append([len(rows) + 1, name, c.get("max_stable_version") or c["newest_version"], c["recent_downloads"] or 0,
                     "lib", cat, "" if features == "-" else features])
    with open(LIST, "w") as h:
        h.write(f"# The top {len(top)} crates of crates.io by recent downloads (the last 90 days) on {datetime.date.today()}, "
                "then extra.tsv's. Columns: rank, name, version (the latest stable), recent downloads, kind (lib, or "
                "other-platform: listed, not probed; the survey finds the proc-macro crates), category (from crates.io's, "
                "see survey.py; may be corrected by hand: `list` keeps it), features (empty: the defaults). "
                "Written by `survey.py list`\n")
        for r in rows:
            h.write("\t".join(map(str, r)) + "\n")
    print(f"{LIST}: {len(rows)} crates")


# ---- lock: one Cargo.lock for every probe ----

def dir_of(r):
    return re.sub(r"[^A-Za-z0-9_-]+", "-", r["id"]).strip("-")


def probe_manifest(r, member=False):
    fs = [f for f in r["features"].split(",") if f]
    dep = f'"={r["version"]}"'
    if fs:
        on = [f for f in fs if f != "-default"]
        dep = (f'{{ version = "={r["version"]}"' + (", default-features = false" if "-default" in fs else "") +
               (f', features = [{", ".join(chr(34) + f + chr(34) for f in on)}]' if on else "") + " }")
    return (f'[package]\nname = "probe-{dir_of(r)}"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n\n'
            f'[lib]\npath = "lib.rs"\n\n[dependencies]\n{r["name"]} = {dep}\n' + ("" if member else "\n[workspace]\n"))


def cmd_lock(a):
    work = a.work or os.path.join(tempfile.gettempdir(), "zust-crate-survey")
    ws = os.path.join(work, "lock")
    shutil.rmtree(ws, ignore_errors=True)
    rows = [r for r in read_list() if r["kind"] != "other-platform"]
    members = []
    for r in rows:
        d = os.path.join(ws, dir_of(r))
        os.makedirs(d)
        open(os.path.join(d, "Cargo.toml"), "w").write(probe_manifest(r, member=True))
        open(os.path.join(d, "lib.rs"), "w").write("")
        members.append(dir_of(r))
    open(os.path.join(ws, "Cargo.toml"), "w").write(
        '[workspace]\nresolver = "2"\nmembers = [\n' + "".join(f'  "{m}",\n' for m in members) + "]\n")
    if os.path.exists(LOCK) and not a.fresh:
        shutil.copy(LOCK, os.path.join(ws, "Cargo.lock"))
    code, _, err, _ = sh(["cargo", "generate-lockfile"] if a.fresh or not os.path.exists(LOCK) else ["cargo", "fetch"], cwd=ws)
    if code != 0:
        sys.exit(f"cargo could not resolve the probes:\n{err[-3000:]}")
    shutil.copy(os.path.join(ws, "Cargo.lock"), LOCK)
    n = open(LOCK).read().count("[[package]]")
    print(f"{LOCK}: {n} packages for {len(members)} probes")


# ---- run ----

@functools.lru_cache(maxsize=None)
def lines_of(d):
    n = 0
    for dp, dirs, fs in os.walk(d):
        dirs[:] = [x for x in dirs if x not in ("target", ".git")]
        for f in fs:
            if f.endswith(".rs"):
                with open(os.path.join(dp, f), errors="ignore") as h:
                    n += sum(1 for _ in h)
    return n


def graph(meta, root_pkg):
    """the packages zustc compiles for the probe (normal dependencies, not through a proc macro), by id"""
    pk = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    seen, st = [], [meta["resolve"]["root"]]
    while st:
        i = st.pop()
        for d in nodes[i]["deps"]:
            j = d["pkg"]
            if j in seen or not any(k["kind"] is None for k in d["dep_kinds"]):
                continue
            if any("proc-macro" in t["kind"] for t in pk[j]["targets"]):
                continue
            seen.append(j)
            st.append(j)
    return [pk[i] for i in seen]


def lib_name(p):
    t = next((t for t in p["targets"] if set(t["kind"]) & {"lib", "rlib", "dylib", "proc-macro"}), None)
    return (t["name"] if t else p["name"]).replace("-", "_")


REG_PATH = re.compile(r"/[^ :]*?/registry/src/[^/]+/([A-Za-z0-9_.+-]+?)-(\d+\.\d+\.\d+[^/]*)/")


def short(msg, dirs):
    """a message with the package's absolute paths made relative (`src/lib.rs:1:2: ..`)"""
    for d in dirs:
        msg = msg.replace(d + "/", "")
    return REG_PATH.sub(lambda m: f"{m.group(1)}-{m.group(2)}/", msg)


def parse_zurgo(err, by_lib, dirs):
    """(errors by package id, packages not checked, a fatal error); `error: crate `x`: msg` lines"""
    errs, unchecked, fatal = {}, {}, None
    for line in err.splitlines():
        m = re.match(r"error: crate `([^`]+)`: (.*)", line)
        if m:
            pid = by_lib.get(m.group(1))
            errs.setdefault(pid or m.group(1), []).append(short(m.group(2), [dirs.get(pid, "")]))
            continue
        m = re.match(r"warning: crate `([^`]+)` not checked: its dependency `([^`]+)`", line)
        if m:
            unchecked[by_lib.get(m.group(1), m.group(1))] = m.group(2)
            continue
        if line.startswith("error: ") and not re.match(r"error: \d+ errors? in \d+ crates?", line) and fatal is None and not errs:
            fatal = short(line[len("error: "):], list(dirs.values()))
    return errs, unchecked, fatal


def compare_tests(cargo_out, zurgo_out):
    """cargo's and zurgo's `test x ... ok` lines, as multisets of (name, result): two targets may each have a
    `tests::smoke`, and neither may hide the other. (cargo's counts, same, differ: cargo's results zurgo does not
    give, and zurgo's that cargo does not)"""
    pat = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)", re.M)
    c, z = collections.Counter(pat.findall(cargo_out)), collections.Counter(pat.findall(zurgo_out))
    same = sum((c & z).values())
    counts = {s: sum(n for (_, v), n in c.items() if v == s) for s in ("ok", "FAILED", "ignored")}
    return counts, same, sum((c - z).values()) + sum((z - c).values())


def feature_flags(features):
    """a row's features as cargo's (and zurgo's) flags"""
    fs = [f for f in features.split(",") if f]
    on = [f for f in fs if f != "-default"]
    return (["--no-default-features"] if "-default" in fs else []) + (["--features", ",".join(on)] if on else [])


def run_tests(r, src_dir, work, jobs):
    """the crate's own tests from its crates.io package, copied to the work directory: cargo test against zurgo test,
    with the row's features"""
    d = os.path.join(work, "tests", dir_of(r))
    shutil.rmtree(d, ignore_errors=True)
    try:
        shutil.copytree(src_dir, d, ignore=shutil.ignore_patterns("target", ".git"))
        # (the package's Cargo.toml is crates.io's normalized one; a [workspace] of its own keeps it out of any parent)
        toml = open(os.path.join(d, "Cargo.toml")).read()
        if "[workspace]" not in toml:
            open(os.path.join(d, "Cargo.toml"), "a").write("\n[workspace]\n")
        args = ["test", "--no-fail-fast", "-j", str(jobs), "--lib", "--bins", "--tests", *feature_flags(r["features"])]
        code, cout, cerr, _ = sh(["cargo", *args], cwd=d, env={"CARGO_TARGET_DIR": os.path.join(d, "target-cargo")})
        if code is None or "test result" not in cout:
            return {"tests": "cargo-fails", "tests_note": (cerr.strip().splitlines() or [""])[-1][:200]}
        code, zout, zerr, _ = sh([ZURGO, *args], cwd=d, env={"CARGO_TARGET_DIR": os.path.join(d, "target-zurgo")})
        counts, same, differ = compare_tests(cout, zout)
        total = sum(counts.values())
        if total == 0:
            return {"tests": "none"}
        if code is None or "test result" not in zout:
            first = next((l for l in zerr.splitlines() if l.startswith("error")), (zerr.strip().splitlines() or [""])[-1])
            return {"tests": "zurgo-fails", "tests_total": total, "tests_note": short(first, [src_dir])[:300]}
        return {"tests": "match" if differ == 0 else "differ", "tests_total": total, "tests_same": same}
    finally:
        # (the copy and both target directories, whatever happened)
        shutil.rmtree(d, ignore_errors=True)


def probe(r, work, jobs, tests):
    t0 = time.time()
    d = os.path.join(work, "probes", dir_of(r))
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    open(os.path.join(d, "Cargo.toml"), "w").write(probe_manifest(r))
    open(os.path.join(d, "lib.rs"), "w").write("")
    shutil.copy(LOCK, os.path.join(d, "Cargo.lock"))
    out = base(r, status=None, graph=0, lines=0, seconds=0)
    packages = {}
    try:
        probe_in(r, d, work, jobs, tests, out, packages)
    finally:
        # (what zurgo compiled is not kept; the logs are)
        shutil.rmtree(os.path.join(d, "target"), ignore_errors=True)
        out["seconds"] = round(time.time() - t0, 1)
    return out, packages


def probe_in(r, d, work, jobs, tests, out, packages):
    name = r["name"]
    # (cargo drops what this probe does not use from the shared lock, and keeps the versions)
    code, meta, err, _ = sh(["cargo", "metadata", "--format-version", "1", "--filter-platform", TARGET], cwd=d)
    if code != 0:
        out.update(status="fetch-fail", first_error=(err.strip().splitlines() or [""])[-1][:300])
        return
    meta = json.loads(meta)
    top = next(p for p in meta["packages"] if p["name"] == name and p["version"] == r["version"] and p["source"])
    src_dir = os.path.dirname(top["manifest_path"])
    out["lines"] = lines_of(os.path.join(src_dir, "src")) or lines_of(src_dir)
    out["build_rs"] = any("custom-build" in t["kind"] for t in top["targets"])
    if any("proc-macro" in t["kind"] for t in top["targets"]):
        out.update(kind="proc-macro", status="skipped")
        return
    pkgs = graph(meta, top)
    out["graph"] = len(pkgs)
    by_lib = {lib_name(p): p["id"] for p in pkgs}
    dirs = {p["id"]: os.path.dirname(p["manifest_path"]) for p in pkgs}
    key = lambda p: f'{p["name"]} {p["version"]}'
    for p in pkgs:
        packages[key(p)] = dict(name=p["name"], version=p["version"], errors=None, unchecked=None,
                                lines=lines_of(os.path.join(dirs[p["id"]], "src")) or lines_of(dirs[p["id"]]))
    env = {"CARGO_TARGET_DIR": os.path.join(d, "target")}
    code, _, err, _ = sh([ZURGO, "check", "-j", str(jobs)], cwd=d, env=env)
    open(os.path.join(d, "check.log"), "w").write(err)
    if code is None:
        out.update(status="timeout", first_error="zurgo check: " + err)
        return
    errs, unchecked, fatal = parse_zurgo(err, by_lib, dirs)
    if fatal and code != 0:
        out.update(status="graph-fail", first_error=fatal[:300])
        return
    for p in pkgs:
        e = errs.get(p["id"], [])
        packages[key(p)]["errors"] = None if p["id"] in unchecked else e
        if p["id"] in unchecked:
            packages[key(p)]["unchecked"] = unchecked[p["id"]]
    own = errs.get(top["id"], [])
    bad = [key(p) for p in pkgs if p["id"] != top["id"] and (errs.get(p["id"]))]
    out["errors"] = len(own)
    out["blocked_by"] = bad
    if own:
        out.update(status="errors", first_error=own[0][:300])
    elif bad or top["id"] in unchecked:
        first = (f'{bad[0]}: {errs[next(p["id"] for p in pkgs if key(p) == bad[0])][0]}' if bad else
                 f'not checked: its dependency `{unchecked[top["id"]]}` could not be read or expanded')
        out.update(status="blocked", first_error=first[:300])
    else:
        code, _, err, _ = sh([ZURGO, "build", "-j", str(jobs)], cwd=d, env=env)
        open(os.path.join(d, "build.log"), "w").write(err)
        if code == 0:
            out["status"] = "builds"
            if tests:
                out.update(run_tests(out, src_dir, work, jobs))
        elif code is None:
            out.update(status="timeout", first_error="zurgo build: " + err)
        else:
            berrs, _, bfatal = parse_zurgo(err, by_lib, dirs)
            first = next(((pid, e[0]) for pid, e in berrs.items() if e), None)
            out["status"] = "build-fail"
            if first:
                pid, msg = first
                who = next((key(p) for p in pkgs if p["id"] == pid), str(pid))
                out["first_error"] = (msg if pid == top["id"] else f"{who}: {msg}")[:300]
                packages.get(who, {})["build_error"] = msg[:300]
            else:
                out["first_error"] = (bfatal or (err.strip().splitlines() or [""])[-1])[:300]


def merge_package(packages, k, v):
    """one package seen by several probes, maybe with other features: every error any of them reports (sorted, so the
    order the probes finish in does not matter); not checked only if no probe checked it"""
    cur = packages.get(k)
    if cur is None:
        packages[k] = dict(v, errors=None if v.get("errors") is None else sorted(set(v["errors"])))
        return
    if v.get("errors") is not None:
        cur["errors"] = sorted(set(cur.get("errors") or []) | set(v["errors"]))
        cur["unchecked"] = None
    if v.get("build_error") and not cur.get("build_error"):
        cur["build_error"] = v["build_error"]


def base(r, **kw):
    """a crate's row of results.json, before or without its probe"""
    return dict(id=r["id"], name=r["name"], version=r["version"], rank=r["rank"], downloads=r["downloads"], kind=r["kind"],
                category=r["category"], features=r["features"], status="skipped", errors=0, first_error="",
                blocked_by=[]) | kw


def cmd_run(a):
    if not os.path.exists(ZURGO):
        sys.exit(f"{ZURGO} is missing: cargo build --release -p zustc -p zurgo first")
    work = a.work or os.path.join(tempfile.gettempdir(), "zust-crate-survey")
    rows = read_list()
    if a.names:
        rows = [r for r in rows if r["name"] in a.names or r["id"] in a.names]
    os.makedirs(work, exist_ok=True)
    old = json.load(open(RESULTS)) if a.names and os.path.exists(RESULTS) else None
    crates, packages = {}, {}
    todo = [r for r in rows if r["kind"] != "other-platform"]
    for r in rows:
        if r["kind"] == "other-platform":
            crates[r["id"]] = base(r)
    jobs = max(1, (os.cpu_count() or 2) // a.parallel)
    t0 = time.time()
    # (the largest graphs first, as far as the last run knows them: the run ends sooner)
    last = json.load(open(RESULTS))["crates"] if os.path.exists(RESULTS) else {}
    todo.sort(key=lambda r: -(last.get(r["id"], {}).get("seconds") or 0))
    with concurrent.futures.ThreadPoolExecutor(a.parallel) as ex:
        futs = {ex.submit(probe, r, work, jobs, a.tests): r for r in todo}
        for f in concurrent.futures.as_completed(futs):
            r = futs[f]
            try:
                c, p = f.result()
            except Exception as e:  # (one crate's surprise is that crate's result, not the run's end)
                c, p = base(r, status="survey-error", first_error=f"survey.py: {e!r}"[:300]), {}
            crates[c["id"]] = c
            for k, v in p.items():
                merge_package(packages, k, v)
            print(f'{c["rank"]:4} {c["id"][:28]:28} {c["status"]:10} {c["errors"]:3} {c.get("tests", "")[:11]:11} '
                  f'{c["first_error"][:90]}', flush=True)
    if old:
        old["crates"].update(crates)
        old["packages"].update(packages)
        crates, packages = old["crates"], old["packages"]
    res = dict(date=datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M UTC"), commit=a.commit or git_head(),
               seconds=round(time.time() - t0), top=n_top(), categories=CATEGORIES,
               crates=dict(sorted(crates.items(), key=lambda kv: kv[1]["rank"])),
               packages=dict(sorted(packages.items())), tests=a.tests or bool(old and old.get("tests")))
    save(res, write_report(res, a.history))


def n_top():
    """how many of crates.tsv's rows are the top N (extra.tsv's come after them)"""
    n_extra = sum(1 for l in open(os.path.join(HERE, "extra.tsv")) if l.strip() and not l.startswith("#"))
    return len(read_list()) - n_extra


def git_head():
    r = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True)
    return r.stdout.strip()


# ---- report ----

def kind_of(msg):
    """an error's kind: its text without the place, names and numbers (`no method `…` on type `…``)"""
    m = re.sub(r"^[^ ]*?\.rs:\d+:\d+: ", "", msg)
    m = re.sub(r"^in [a-z]+ `[^`]*`: ", "", m)
    m = re.sub(r"`[^`]*`", "`…`", m)
    m = re.sub(r"\b\d+\b", "N", m)
    m = re.sub(r"\(found [^)]*\)", "(found …)", m)
    return m.strip()[:120]


def summarize(res):
    C = [c for c in res["crates"].values() if c["kind"] == "lib"]
    P = res["packages"]
    dl = sum(c["downloads"] for c in C) or 1
    count = {s: [c for c in C if c["status"] == s] for s in STATUSES}
    with_err = {k: v for k, v in P.items() if v.get("errors")}
    return dict(C=C, P=P, dl=dl, count=count, with_err=with_err,
                n_err=sum(len(v["errors"]) for v in with_err.values()),
                builds=len(count["builds"]), builds_dl=sum(c["downloads"] for c in count["builds"]))


def priorities(res):
    """for each package with errors: the surveyed crates that need it, and those it alone keeps from building"""
    s = summarize(res)
    rows = []
    for k, v in s["with_err"].items():
        need = [c for c in s["C"] if c["status"] in ("errors", "blocked") and
                (f'{c["name"]} {c["version"]}' == k or k in c["blocked_by"])]
        alone = [c for c in need if (c["blocked_by"] == [k] and c["errors"] == 0) or
                 (f'{c["name"]} {c["version"]}' == k and not c["blocked_by"])]
        rows.append(dict(package=k, errors=len(v["errors"]), lines=v.get("lines", 0), need=need, alone=alone,
                         first=v["errors"][0]))
    rows.sort(key=lambda r: (-sum(c["downloads"] for c in r["alone"]), -len(r["need"]), r["errors"], r["package"]))
    return rows


def kinds(res):
    s = summarize(res)
    K = {}
    for k, v in s["with_err"].items():
        for e in v["errors"]:
            g = K.setdefault(kind_of(e), dict(errors=0, packages=set(), example=f"{k}: {e}"))
            g["errors"] += 1
            g["packages"].add(k)
    return sorted(K.items(), key=lambda kv: (-len(kv[1]["packages"]), -kv[1]["errors"], kv[0]))


MISSING = re.compile(r"cannot find (type|trait|value|derive macro|associated type|macro|function) `([^`]+)`")


@functools.lru_cache(maxsize=None)
def package_dir(key):
    """a package's sources, `name version`, in cargo's registry (None when they are not there)"""
    name, version = key.split(" ")
    for d in glob.glob(os.path.join(os.environ.get("CARGO_HOME") or os.path.expanduser("~/.cargo"), "registry/src/*", f"{name}-{version}")):
        return d
    return None


@functools.lru_cache(maxsize=None)
def defines(key, name):
    """whether the package's own code defines the name (zustc's mangled `a__b` names are its own items); None: its
    sources are not there"""
    if "__" in name.strip("_"):
        return True
    d = package_dir(key)
    if d is None:
        return None
    pat = re.compile(rf"\b(struct|enum|union|type|trait|const|static|fn|mod|macro_rules!)\s+{re.escape(name)}\b")
    for dp, dirs, fs in os.walk(d):
        dirs[:] = [x for x in dirs if x not in ("target", ".git", "tests", "benches", "examples")]
        for f in fs:
            if f.endswith(".rs"):
                with open(os.path.join(dp, f), errors="ignore") as h:
                    if pat.search(h.read()):
                        return True
    return False


def missing(res):
    """the names zustc cannot find: std's (the prelude lacks them) and the crates' own (resolution lost them;
    zustc's names for a crate's items have `__` in them)"""
    M = {}
    for k, v in summarize(res)["with_err"].items():
        for e in v["errors"]:
            m = MISSING.search(e)
            if m:
                g = M.setdefault((m.group(1), m.group(2)), set())
                g.add(k)
    # (without the packages' sources (a report from results.json alone) a name keeps the class the run gave it)
    known = {(m["what"], m["name"]): m.get("own") for m in res.get("missing") or []}
    rows = []
    for (w, n), ps in M.items():
        found = [defines(p, n) for p in ps]
        own = True if any(found) else (False if all(f is False for f in found) else known.get((w, n)))
        rows.append(dict(what=w, name=n, packages=sorted(ps), own=own))
    rows.sort(key=lambda r: (-len(r["packages"]), r["name"]))
    return rows


def graph_reasons(res):
    """why zurgo cannot plan the graphs it cannot: by message without names"""
    G = {}
    for c in summarize(res)["count"]["graph-fail"]:
        k = re.sub(r"`[^`]*`", "`…`", c["first_error"])
        G.setdefault(k, []).append(c)
    return sorted(G.items(), key=lambda kv: (-len(kv[1]), kv[0]))


ISSUES = os.path.join(HERE, "issues.tsv")


@functools.lru_cache(maxsize=None)
def issue_rules():
    """issues.tsv: (regex, issue or None, the fix's name), in order"""
    rows = []
    if os.path.exists(ISSUES):
        for line in open(ISSUES):
            if line.startswith("#") or not line.strip():
                continue
            pat, num, title = line.rstrip("\n").split("\t")
            rows.append((re.compile(pat), int(num) if num.strip() else None, title))
    return tuple(rows)


def fix_of(msg, own):
    """an error's fix: (its name, its issue or None). `own`: missing()'s classes of the names zustc cannot find"""
    for pat, num, title in issue_rules():
        if pat.search(msg):
            return title, num
    m = MISSING.search(msg)
    if m:
        o = own.get((m.group(1), m.group(2)))
        if o is False:
            return f"prelude: {m.group(1)} `{m.group(2)}`", 250
        if o is True:
            return "a crate's own item zustc cannot find (resolution, cfg, a macro's output)", 254
    return kind_of(msg), None


def fixes(res):
    """What to fix, by importance. Each surveyed crate that does not build needs a set of fixes: those of every error
    in its graph (its own and its dependencies'), or of why zurgo cannot plan or build it. Its downloads are shared
    equally among them, and a fix's score is the sum of its shares: what it holds back, as far as zustc shows. A crate
    that needs one fix only is *alone* for it: fixing it makes the crate check (or plan, or build), until the next error
    shows"""
    s = summarize(res)
    own = {(m["what"], m["name"]): m.get("own") for m in missing(res)}
    F = {}

    def entry(key, num, example):
        return F.setdefault(key, dict(fix=key, issue=num, score=0.0, crates=[], alone=[], packages=set(), errors=0,
                                      example=example))
    pkg_fixes = {}
    for k, v in s["with_err"].items():
        for e in v["errors"]:
            key, num = fix_of(e, own)
            g = entry(key, num, f"{k}: {e}")
            g["packages"].add(k)
            g["errors"] += 1
            pkg_fixes.setdefault(k, set()).add(key)
    for c in sorted(s["C"], key=lambda c: c["rank"]):
        need = set()
        if c["status"] in ("graph-fail", "build-fail", "timeout"):
            msg = re.sub(r"^[\w.+-]+ \d[^:]*: ", "", c["first_error"])
            key, num = fix_of(msg, own)
            if num is None and c["status"] == "build-fail":
                key = "build: " + key
            g = entry(key, num, f'{c["id"]}: {c["first_error"]}')
            g["packages"].add(f'{c["name"]} {c["version"]}')
            g["errors"] += 1
            need.add(key)
        elif c["status"] in ("errors", "blocked"):
            for p in [f'{c["name"]} {c["version"]}'] + list(c.get("blocked_by") or []):
                need |= pkg_fixes.get(p, set())
        for key in need:
            F[key]["score"] += c["downloads"] / len(need)
            F[key]["crates"].append(c["id"])
            if len(need) == 1:
                F[key]["alone"].append(c["id"])
    rows = sorted(F.values(), key=lambda g: (-g["score"], -len(g["packages"]), g["fix"]))
    for g in rows:
        g["packages"] = sorted(g["packages"])
        g["share"] = round(100 * g["score"] / s["dl"], 2)
        g["score"] = round(g["score"])
    return rows


def cell(s, n=140):
    s = " ".join((s or "").split()).replace("|", "\\|")
    return (s[:n] + "…") if len(s) > n else s


def pct(a, b):
    return f"{100 * a / b:.1f}%" if b else "—"


def write_report(res, history):
    s = summarize(res)
    C, count, dl = s["C"], s["count"], s["dl"]
    allc = list(res["crates"].values())
    n_pm = sum(c["kind"] == "proc-macro" for c in allc)
    n_os = sum(c["kind"] == "other-platform" for c in allc)
    o = ["# Crate support", "",
         "<!-- Generated by cratebench/survey/survey.py from cratebench/survey/results.json: do not edit by hand -->", "",
         f"Which of crates.io's most used crates zustc builds today: the top {res.get('top', len(allc))} crates by recent "
         f"downloads and {len(allc) - res.get('top', len(allc))} more ([crates.tsv](cratebench/survey/crates.tsv), "
         f"[extra.tsv](cratebench/survey/extra.tsv): tokio with all its features, embassy, embedded crates), each one "
         f"as the only dependency of a probe library with its default features (or the ones named), built by zurgo for "
         f"x86_64 Linux. Measured {res['date']} on `{res['commit']}` "
         f"in {res['seconds'] // 60} min. The same data, with its history: the "
         f"[dashboard](https://mizchi.github.io/zust/crates/). How it is made: [cratebench/survey/](cratebench/survey/README.md).", "",
         "## Summary", "",
         "| status | crates | share of their downloads | meaning |", "|---|---:|---:|---|"]
    meaning = {"builds": "zurgo builds it and every dependency",
               "build-fail": "the check finds nothing, the build fails (code generation, linking)",
               "blocked": "its own code checks (or is not read), a dependency's does not",
               "errors": "zustc reports errors in its own code",
               "graph-fail": "zurgo cannot plan its graph (two versions of a crate, mizchi/zust#28, and others)",
               "timeout": f"over {TIMEOUT} s",
               "fetch-fail": "cargo could not resolve or fetch it",
               "survey-error": "survey.py itself failed on it (a bug of the survey)"}
    for st in STATUSES:
        v = count[st]
        if v or st in ("builds", "errors", "blocked"):
            o.append(f"| {st} | {len(v)} / {len(C)} ({pct(len(v), len(C))}) | {pct(sum(c['downloads'] for c in v), dl)} | {meaning[st]} |")
    o += ["",
          f"- Not probed: {n_pm} proc-macro crates (rustc builds them for the host; the crates that use them test them) "
          f"and {n_os} crates for other platforms.",
          f"- Errors zustc reports in the probes' graphs: **{s['n_err']}** in {len(s['with_err'])} of "
          f"{len(s['P'])} packages. A lower bound: zustc stops reading a crate at a syntax or macro error, and checks "
          "each function only up to its first error, so fixing one can show more."]
    if res.get("tests"):
        t = [c for c in C if c.get("tests")]
        m = [c for c in t if c["tests"] == "match"]
        o.append(f"- Own tests (`--tests`, the crates that build): {len(m)} of {len(t)} give the same result as "
                 "`cargo test`, test by test.")
    o += ["", "## By category", "",
          "Crates of one kind tend to stop zustc in the same places. *Own errors*: the errors in the category's crates' "
          "own code; the kinds are the most common among them.", "",
          "| category | crates | builds | build-fail | blocked | errors | graph-fail | own errors | most common kinds |",
          "|---|---:|---:|---:|---:|---:|---:|---:|---|"]
    for cat, label in res.get("categories", CATEGORIES).items():
        cs = [c for c in C if c.get("category", "other") == cat]
        if not cs:
            continue
        n = lambda st: sum(c["status"] == st for c in cs) or ""
        ks = {}
        for c in cs:
            for e in (s["P"].get(f'{c["name"]} {c["version"]}', {}).get("errors") or []):
                ks[kind_of(e)] = ks.get(kind_of(e), 0) + 1
        top = "; ".join(f"{k} ({v})" for k, v in sorted(ks.items(), key=lambda kv: (-kv[1], kv[0]))[:2])
        o.append(f"| {label} | {len(cs)} | {n('builds')} | {n('build-fail')} | {n('blocked')} | {n('errors')} | "
                 f"{n('graph-fail')} | {sum(c['errors'] for c in cs) or ''} | {cell(top, 140)} |")
    fx = fixes(res)
    o += ["", "## What to fix, by importance", "",
          "Every error, grouped by the fix it needs ([issues.tsv](cratebench/survey/issues.tsv) names the fixes with a "
          "tracking issue; the rest are grouped by their text without names), and ordered by what the fix holds back. "
          "Each surveyed crate that does not build shares its downloads equally among the fixes its graph needs (every "
          "error in it, its own and its dependencies', or why zurgo cannot plan or build it); a fix's share is the sum. "
          "*Alone*: the crates that need only this fix, as far as zustc shows (it stops a crate at its first item "
          "error, so another can come next).", "",
          "| # | fix | issue | holds back | crates | alone | packages | errors | example |",
          "|---:|---|---|---:|---:|---|---:|---:|---|"]
    for n, g in enumerate(fx[:40], 1):
        al = ", ".join(g["alone"][:5]) + (" …" if len(g["alone"]) > 5 else "")
        iss = f"#{g['issue']}" if g["issue"] else "—"
        o.append(f"| {n} | {cell(g['fix'], 90)} | {iss} | {g['share']:.1f}% | {len(g['crates'])} | "
                 f"{len(g['alone'])}{': ' + al if al else ''} | {len(g['packages'])} | {g['errors']} | {cell(g['example'], 100)} |")
    no_issue = [g for g in fx if not g["issue"]]
    o += ["", f"{len(fx)} fixes; {len(no_issue)} have no tracking issue yet "
              f"(together {sum(g['share'] for g in no_issue):.1f}%). The top ones without an issue are the next to file."]
    o += ["", "## Packages with errors", "",
          "Packages with errors, ordered by the downloads of the surveyed crates that only they keep from checking "
          "(*alone*: no other package of the crate's graph has errors), then by how many surveyed crates need them. "
          "Errors are what zustc reports now; lines are the package's `.rs` lines.", "",
          "| package | errors | lines | alone | needed by | first error |", "|---|---:|---:|---|---:|---|"]
    for r in priorities(res)[:40]:
        al = ", ".join(c["name"] for c in sorted(r["alone"], key=lambda c: c["rank"])[:6]) + (" …" if len(r["alone"]) > 6 else "")
        o.append(f"| {r['package']} | {r['errors']} | {r['lines']} | {len(r['alone'])}{': ' + al if al else ''} | "
                 f"{len(r['need'])} | {cell(r['first'], 110)} |")
    gr = graph_reasons(res)
    if gr:
        o += ["", "## Graphs zurgo cannot plan", "",
              "zurgo's own gaps, before zustc reads anything: each one stops every crate whose graph has it.", "",
              "| reason | crates | crates (by rank) |", "|---|---:|---|"]
        for k, cs in gr:
            names = ", ".join(c["id"] for c in sorted(cs, key=lambda c: c["rank"]))
            o.append(f"| {cell(k, 150)} | {len(cs)} | {cell(names, 160)} |")
    mi = missing(res)
    if mi:
        o += ["", "## Names zustc cannot find", "",
              "*std*: a name the crate does not define, from the standard library (or `proc_macro`), that zustc's prelude "
              "does not have yet: each is a small addition. *crate*: one of the crate's own items, which zustc lost "
              "(name resolution, `cfg`, a macro's output).", "",
              "| name | of | kind | packages |", "|---|---|---|---|"]
        for r in mi[:50]:
            o.append(f"| `{r['name']}` | {'?' if r['own'] is None else 'crate' if r['own'] else 'std'} | {r['what']} | "
                     f"{cell(', '.join(r['packages']), 120)} |")
    o += ["", "## Errors by kind", "",
          "Each error's text without its place, names and numbers. A kind in many packages is one gap of zustc's.", "",
          "| kind | packages | errors | example |", "|---|---:|---:|---|"]
    for k, g in kinds(res)[:40]:
        o.append(f"| {cell(k, 90)} | {len(g['packages'])} | {g['errors']} | {cell(g['example'], 120)} |")
    o += ["", "## All crates", "", "By category, then by rank (the top N's; extra.tsv's come after). *Own errors*: in the "
          "crate's own code; *blocked by*: the packages of its graph with errors."]
    for cat, label in res.get("categories", CATEGORIES).items():
        cs = sorted((c for c in allc if c.get("category", "other") == cat), key=lambda c: c["rank"])
        if not cs:
            continue
        o += ["", f"### {label}", "", "| # | crate | status | own errors | blocked by | first error |", "|---:|---|---|---:|---|---|"]
        for c in cs:
            st = c["status"] if c["kind"] == "lib" else c["kind"]
            if c.get("tests"):
                st += f" (tests: {c['tests']})"
            bb = ", ".join(c.get("blocked_by", [])[:3]) + (" …" if len(c.get("blocked_by", [])) > 3 else "")
            feats = f" [{c['features']}]" if c.get("features") else ""
            o.append(f"| {c['rank']} | {c['name']} {c['version']}{feats} | {st} | {c['errors'] or ''} | {cell(bb, 60)} | "
                     f"{cell(c['first_error'], 100)} |")
    open(DOC, "w").write("\n".join(o) + "\n")
    write_todo(res, s, fx)
    print(f"{DOC}: {s['builds']} / {len(C)} build; {s['n_err']} errors in {len(s['with_err'])} packages")
    derived = dict(
        priorities=[dict(package=r["package"], errors=r["errors"], lines=r["lines"], first=r["first"],
                         alone=[c["id"] for c in sorted(r["alone"], key=lambda c: c["rank"])],
                         alone_downloads=sum(c["downloads"] for c in r["alone"]), needed_by=len(r["need"]))
                    for r in priorities(res)[:100]],
        kinds=[dict(kind=k, packages=sorted(g["packages"]), errors=g["errors"], example=g["example"]) for k, g in kinds(res)[:100]],
        fixes=fx[:150],
        missing=missing(res)[:100],
        graph_reasons=[dict(reason=k, crates=[c["id"] for c in sorted(cs, key=lambda c: c["rank"])]) for k, cs in graph_reasons(res)])
    if history:
        new = not os.path.exists(HISTORY)
        with open(HISTORY, "a") as h:
            if new:
                h.write("# date\tcommit\tsurveyed\tbuilds\tbuild_fail\tblocked\terrors\tgraph_fail\tbuilds_downloads_pct\t"
                        "error_count\tpackages_with_errors\n")
            h.write("\t".join(map(str, [res["date"], res["commit"], len(C), s["builds"], len(count["build-fail"]),
                                         len(count["blocked"]), len(count["errors"]), len(count["graph-fail"]),
                                         f"{100 * s['builds_dl'] / dl:.2f}", s["n_err"], len(s["with_err"])])) + "\n")
    return derived


def save(res, derived):
    """results.json: the probes' results, and what the report derives from them (the dashboard reads both)"""
    res.update(derived)
    # (one line per crate, package, priority and kind: a survey's diff shows what changed, line by line)
    with open(RESULTS, "w") as h:
        h.write("{\n")
        keys = list(res)
        for n, k in enumerate(keys):
            v = res[k]
            end = ",\n" if n < len(keys) - 1 else "\n"
            if isinstance(v, dict) and k in ("crates", "packages"):
                h.write(f"{json.dumps(k)}: {{\n" + ",\n".join(f" {json.dumps(kk)}: {json.dumps(vv, ensure_ascii=False)}"
                                                         for kk, vv in v.items()) + "\n}" + end)
            elif isinstance(v, list):
                h.write(f"{json.dumps(k)}: [\n" + ",\n".join(f" {json.dumps(x, ensure_ascii=False)}" for x in v) + "\n]" + end)
            else:
                h.write(f"{json.dumps(k)}: {json.dumps(v, ensure_ascii=False)}" + end)
        h.write("}\n")


def tier_of(share):
    """an issue's tier from the share of the surveyed downloads its fixes hold back together"""
    return "T0" if share >= 2 else "T1" if share >= 1 else "T2"


def write_todo(res, s, fx):
    """TODO.md's "Crate coverage: what to fix next": the top fixes, between its markers (nothing when they are not there)"""
    if not os.path.exists(TODO):
        return
    text = open(TODO).read()
    start, end = "<!-- crate-priorities -->", "<!-- /crate-priorities -->"
    if start not in text or end not in text:
        return
    url = "https://github.com/mizchi/zust/issues/"
    o = [start, "",
         f"Survey of {res['date']} on `{res['commit']}`: {s['builds']} of {len(s['C'])} crates build "
         f"({pct(s['builds_dl'], s['dl'])} of their downloads); {s['n_err']} errors in {len(s['with_err'])} packages.", "",
         "| # | fix | issue | holds back | crates it alone unblocks |", "|---:|---|---|---:|---:|"]
    for n, g in enumerate(fx[:12], 1):
        iss = f"[#{g['issue']}]({url}{g['issue']})" if g["issue"] else "none yet"
        o.append(f"| {n} | {cell(g['fix'], 80)} | {iss} | {g['share']:.1f}% | {len(g['alone'])} |")
    # (by issue: the issues' tiers, the labels T0 / T1 / T2 on GitHub)
    tot = {}
    for g in fx:
        if g["issue"]:
            t = tot.setdefault(g["issue"], dict(share=0.0, fixes=[]))
            t["share"] += g["share"]
            t["fixes"].append(g["fix"])
    o += ["", "By issue, with its tier (the issue's label: **T0** holds back 2% or more of the surveyed downloads with "
              "all its fixes, **T1** 1% or more, **T2** less; relabel an issue when its tier here changes):", "",
          "| tier | issue | holds back | fixes | the largest |", "|---|---|---:|---:|---|"]
    for num, t in sorted(tot.items(), key=lambda kv: -kv[1]["share"]):
        o.append(f"| {tier_of(t['share'])} | [#{num}]({url}{num}) | {t['share']:.1f}% | {len(t['fixes'])} | "
                 f"{cell(t['fixes'][0], 70)} |")
    nxt = [g for g in fx if not g["issue"]][:5]
    if nxt:
        o += ["", "No issue yet, the next to file: " + "; ".join(f"{cell(g['fix'], 60)} ({g['share']:.1f}%)" for g in nxt) + "."]
    o += ["", end]
    a, b = text.index(start), text.index(end) + len(end)
    open(TODO, "w").write(text[:a] + "\n".join(o) + text[b:])


def cmd_report(a):
    res = json.load(open(RESULTS))
    res.setdefault("top", n_top())
    res.setdefault("categories", CATEGORIES)
    if a.commit:
        res["commit"] = a.commit
    save(res, write_report(res, a.history))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("list")
    p.add_argument("--top", type=int, default=300)
    p = sub.add_parser("lock")
    p.add_argument("--work")
    p.add_argument("--fresh", action="store_true", help="resolve every version again (the latest that fit)")
    p = sub.add_parser("run")
    p.add_argument("names", nargs="*")
    p.add_argument("-j", "--parallel", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    p.add_argument("--tests", action="store_true")
    p.add_argument("--work")
    p.add_argument("--commit")
    p.add_argument("--history", action="store_true")
    p = sub.add_parser("report")
    p.add_argument("--commit")
    p.add_argument("--history", action="store_true")
    a = ap.parse_args()
    {"list": cmd_list, "lock": cmd_lock, "run": cmd_run, "report": cmd_report}[a.cmd](a)


if __name__ == "__main__":
    main()
