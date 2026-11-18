window.BENCHMARK_DATA = {
  "lastUpdate": 1794987824644,
  "repoUrl": "https://github.com/mizchi/zust",
  "entries": {
    "zustc": [
      {
        "commit": {
          "author": {
            "email": "noreply@anthropic.com",
            "name": "Claude",
            "username": "claude"
          },
          "committer": {
            "email": "noreply@anthropic.com",
            "name": "Claude",
            "username": "claude"
          },
          "distinct": true,
          "id": "268b819b8ef9ee9bd864f1f329e8448ab19d55d9",
          "message": "ci: GitHub Actions for tests and a continuous benchmark dashboard (github-action-benchmark on gh-pages)\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01JroQ9gGLQPao3UW4tGGgZE",
          "timestamp": "2026-11-18T07:39:00Z",
          "tree_id": "cd9270b8e726f86d122891679346117e44652d8b",
          "url": "https://github.com/mizchi/zust/commit/268b819b8ef9ee9bd864f1f329e8448ab19d55d9"
        },
        "date": 1794987824102,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "gen1000 build cold",
            "value": 94.667,
            "unit": "ms"
          },
          {
            "name": "gen1000 build noop",
            "value": 15.037,
            "unit": "ms"
          },
          {
            "name": "gen1000 build edit 1 fn",
            "value": 20.503,
            "unit": "ms"
          },
          {
            "name": "gen1000 check",
            "value": 77.145,
            "unit": "ms"
          },
          {
            "name": "gen1000 build cold peak RSS",
            "value": 19.965,
            "unit": "MiB"
          },
          {
            "name": "gen1000 binary",
            "value": 323.977,
            "unit": "KiB"
          },
          {
            "name": "gen1000 cache",
            "value": 376.264,
            "unit": "KiB"
          },
          {
            "name": "gen1000 build noop instructions",
            "value": 155.009,
            "unit": "M instr"
          },
          {
            "name": "gen1000 build edit 1 fn instructions",
            "value": 147.647,
            "unit": "M instr"
          },
          {
            "name": "gen1000 check instructions",
            "value": 762.935,
            "unit": "M instr"
          },
          {
            "name": "gen5000 build cold",
            "value": 429.951,
            "unit": "ms"
          },
          {
            "name": "gen5000 build noop",
            "value": 87.639,
            "unit": "ms"
          },
          {
            "name": "gen5000 build edit 1 fn",
            "value": 92.73,
            "unit": "ms"
          },
          {
            "name": "gen5000 check",
            "value": 397.641,
            "unit": "ms"
          },
          {
            "name": "gen5000 build cold peak RSS",
            "value": 75.492,
            "unit": "MiB"
          },
          {
            "name": "gen5000 binary",
            "value": 1558.336,
            "unit": "KiB"
          },
          {
            "name": "gen5000 cache",
            "value": 1848.92,
            "unit": "KiB"
          },
          {
            "name": "gen5000 build noop instructions",
            "value": 745.353,
            "unit": "M instr"
          },
          {
            "name": "gen5000 build edit 1 fn instructions",
            "value": 703.467,
            "unit": "M instr"
          },
          {
            "name": "gen5000 check instructions",
            "value": 3778.979,
            "unit": "M instr"
          },
          {
            "name": "rt/fib run (-O)",
            "value": 37.271,
            "unit": "ms"
          },
          {
            "name": "rt/hashmap run (-O)",
            "value": 130.901,
            "unit": "ms"
          },
          {
            "name": "rt/matmul run (-O)",
            "value": 31.019,
            "unit": "ms"
          },
          {
            "name": "rt/nbody run (-O)",
            "value": 53.695,
            "unit": "ms"
          },
          {
            "name": "rt/sieve run (-O)",
            "value": 62.743,
            "unit": "ms"
          },
          {
            "name": "rt/sort run (-O)",
            "value": 167.424,
            "unit": "ms"
          },
          {
            "name": "rt/strings run (-O)",
            "value": 43.678,
            "unit": "ms"
          }
        ]
      }
    ]
  }
}