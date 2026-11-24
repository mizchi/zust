"""Run two fresh Codex executors per case; stage.py + collect.py are the subagent adapter."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

here = Path(__file__).resolve().parent
parser = argparse.ArgumentParser()
parser.add_argument("work", type=Path, help="new directory outside the Git checkout")
parser.add_argument("records", type=Path, help="new directory for verification records")
parser.add_argument("--guide", type=Path, default=here.parents[1] / "README.md")
parser.add_argument("--zsc", type=Path, required=True)
parser.add_argument("--zustc", type=Path, required=True)
parser.add_argument("--cases", nargs="+", choices=["status", "energy", "aspect"], default=["status", "energy"])
args = parser.parse_args()
work = args.work.resolve()
repo = here.parents[3]
if work.is_relative_to(repo):
    parser.error("choose a work directory outside the repository")
if work.exists() or args.records.exists():
    parser.error("work and records must both be new directories")
records = args.records.resolve()
if records == work or work in records.parents or records in work.parents:
    parser.error("keep grading records separate from the executors' work directories")
work.mkdir(parents=True)
records.mkdir(parents=True)
had_failure = False
for case in args.cases:
    for n in (1, 2):
        key = f"{case}-{n}"
        d = work / key
        subprocess.run([sys.executable, here / "stage.py", case, d, "--guide", args.guide,
                        "--zsc", args.zsc, "--zustc", args.zustc], check=True)
        with (work / f"{key}.log").open("w") as log:
            run = subprocess.run(["codex", "exec", "--ephemeral", "--skip-git-repo-check",
                                  "--sandbox", "workspace-write", "--cd", d,
                                  "TASK.md と CASE.md に従って試行してください。この作業ディレクトリ内のみを参照してください。"],
                                 cwd=d, stdout=log, stderr=subprocess.STDOUT)
        if run.returncode:
            print(f"{key}: executor exited {run.returncode}, see {work / (key + '.log')}", file=sys.stderr)
            had_failure = True
        target = records / f"{key}.json"
        subprocess.run([sys.executable, here / "collect.py", case, d, target], check=True)
        had_failure |= not json.loads(target.read_text())["verification"]["passed"]
sys.exit(int(had_failure))
