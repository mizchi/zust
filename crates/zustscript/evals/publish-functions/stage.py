"""Create a fresh, self-contained working directory for a single executor."""
import argparse
from pathlib import Path
from shutil import copy2

here = Path(__file__).resolve().parent
parser = argparse.ArgumentParser()
parser.add_argument("case", choices=["status", "energy", "aspect"])
parser.add_argument("directory", type=Path)
parser.add_argument("--zsc", type=Path, required=True)
parser.add_argument("--zustc", type=Path, required=True)
parser.add_argument("--guide", type=Path, default=here.parents[1] / "README.md")
args = parser.parse_args()
dest = args.directory
dest.mkdir(parents=True, exist_ok=False)
for path, name in [(args.guide, "README.md"), (here / "task.md", "TASK.md"),
                   (here / "cases" / (args.case + ".md"), "CASE.md"),
                   (args.zsc, "zsc"), (args.zustc, "zustc")]:
    copy2(path, dest / name)
fixture = here / "cases" / (args.case + ".ts")
if fixture.is_file():
    copy2(fixture, dest / "model.ts")
runner = dest / "compile"
runner.write_text("""#!/bin/sh
set -eu
cd "$(dirname "$0")"
mkdir -p attempts
n=1
while [ -d "attempts/$n" ]; do n=$((n+1)); done
[ "$n" -le 3 ] || { echo 'compile budget exhausted' >&2; exit 1; }
mkdir "attempts/$n"
cp answer.zsc "attempts/$n/"
rc=0
./zsc answer.zsc --zustc ./zustc -o out > "attempts/$n/compiler.log" 2>&1 || rc=$?
cat "attempts/$n/compiler.log"
echo "$rc" > "attempts/$n/exit-code"
exit "$rc"
""")
runner.chmod(0o755)
