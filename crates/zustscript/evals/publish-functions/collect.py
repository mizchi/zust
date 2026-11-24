"""Collect a finished trial, including independent checks, into a portable JSON record."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

here = Path(__file__).resolve().parent
parser = argparse.ArgumentParser()
parser.add_argument("case", choices=["status", "energy", "aspect"])
parser.add_argument("directory", type=Path)
parser.add_argument("record", type=Path)
args = parser.parse_args()
d = args.directory.resolve()


def read(path):
    return path.read_text() if path.is_file() else None


def check(source):
    command = ["node", str(here / "verify.mjs"), args.case, source, d / "zsc", d / "zustc"]
    run = subprocess.run(command, capture_output=True, text=True)
    if run.returncode not in (0, 1) or not run.stdout:
        raise RuntimeError(run.stderr or "verifier stopped without a result")
    return json.loads(run.stdout)


attempts = []
for a in sorted((d / "attempts").glob("*/"), key=lambda p: int(p.name)):
    attempts.append({"source": read(a / "answer.zsc"), "exit_code": read(a / "exit-code"),
                     "compiler_log": read(a / "compiler.log")})
answer = read(d / "answer.zsc")
hashes = {name: hashlib.sha256((d / name).read_bytes()).hexdigest()
          for name in ["TASK.md", "CASE.md", "README.md", "zsc", "zustc", "model.ts"]
          if (d / name).exists()}
result = {
    "case": args.case, "hashes_sha256": hashes, "attempts": attempts, "answer": answer,
    "executor_test": read(d / "check.mjs") or read(d / "verify.mjs"), "feedback": read(d / "feedback.md"),
    "verification": check(d / "answer.zsc") if answer is not None else
                    {"passed": False, "checks": [{"name": "submission", "passed": False}]},
}
args.record.parent.mkdir(parents=True, exist_ok=True)
with args.record.open("x") as dest:
    json.dump(result, dest, ensure_ascii=False, indent=2)
    dest.write("\n")
print(args.record, "PASS" if result["verification"]["passed"] else "FAIL", len(attempts), "attempt(s)")
