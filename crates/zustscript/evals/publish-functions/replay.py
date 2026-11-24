"""Recompile the archived final submissions and check behavior with the supplied toolchain."""
import json
import subprocess
import sys
import tempfile
from pathlib import Path

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
        run = subprocess.run(["node", check, data["case"], answer, zsc, zustc, *model], capture_output=True, text=True)
        print(path.name, "PASS" if run.returncode == 0 else run.stdout or run.stderr)
        failed |= run.returncode != 0
sys.exit(int(failed))
