"""Recompile the archived final submissions and check behavior with the supplied toolchain."""
import json
import os
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

# (seconds for one submission: compiling it and running its checks; VERIFY_TIMEOUT to change it)
TIMEOUT = int(os.environ.get("VERIFY_TIMEOUT", "120"))


def run_verifier(command, timeout, tmp):
    """The verifier's result, or None when it gives none within timeout seconds. It runs in a process group of its own:
    on a timeout the group goes, the compilers the verifier started included, before tmp (its temporary files) does"""
    p = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True,
                         env={**os.environ, "TMPDIR": tmp})
    try:
        out, err = p.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGKILL)
        p.communicate()
        return None
    return subprocess.CompletedProcess(command, p.returncode, out, err)

folder, zsc, zustc, *model = sys.argv[1:]
if len(model) > 1:
    sys.exit("expected at most one alternate model.ts")
records = sorted(Path(folder).glob("*.json"))
if not records:
    sys.exit("no trial JSON files found")
failed = False
with tempfile.TemporaryDirectory() as work:
    answer = Path(work) / "answer.zsc"
    for path in records:
        data = json.loads(path.read_text())
        if data["answer"] is None:
            print(path.name, "missing submission")
            failed = True
            continue
        answer.write_text(data["answer"])
        check = Path(__file__).with_name("verify.mjs")
        # (a submission that never returns, a loop that does not advance: a failure, and the next one is checked. Its
        #  temporary files are under work, removed with it)
        run = run_verifier(["node", check, data["case"], answer, zsc, zustc, *model], TIMEOUT, work)
        if run is None:
            print(path.name, f"FAIL: no result within {TIMEOUT} s")
            failed = True
            continue
        print(path.name, "PASS" if run.returncode == 0 else run.stdout or run.stderr)
        failed |= run.returncode != 0
sys.exit(int(failed))
