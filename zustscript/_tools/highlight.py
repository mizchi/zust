"""Refresh the marked <pre><code> blocks in the zustscript article in place."""
import html
import json
from pathlib import Path
import re
import subprocess
import sys

work = Path(sys.argv[1])
page = Path(__file__).resolve().parents[1] / "index.html"
config = work / "config.json"
config.write_text(json.dumps({"parser-directories": [str(work / "node_modules")]}))
scopes = {"zustscript": "zustscript", "typescript": "ts", "javascript": "js", "bash": "bash"}


def source(markup):
    return html.unescape(re.sub(r"</?span\b[^>]*>", "", markup))


def highlight(block):
    lang, markup = block.groups()
    code = source(markup)
    snippet = work / ("example." + lang)
    snippet.write_text(code)
    rendered = subprocess.check_output([
        "tree-sitter", "highlight", "--html", "--css-classes", "--config-path", str(config),
        "--scope", "source." + scopes[lang], str(snippet),
    ], text=True, cwd=work)
    # The CLI wraps each highlighted source line in a table cell.
    lines = re.findall(r"<td class=line>(.*?)</td>", rendered, re.S)
    colored = "".join(lines)
    if not code.endswith("\n"):
        colored = colored.removesuffix("\n")
    if not lines or source(colored) != code:
        raise ValueError(f"Tree-sitter output did not preserve the {lang} snippet")
    return f'<pre><code class="language-{lang}">{colored}</code></pre>'


page.write_text(re.sub(r'<pre><code class="language-([\w-]+)">(.*?)</code></pre>',
                       highlight, page.read_text(), flags=re.S))
