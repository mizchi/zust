blaze 0.20261027.0 (49ca152) / zustc d9294dc / tests/ui at rustc 1.100.0: 300 applicable tests of each kind (all of a kind with
fewer; seed 1), the same tests for both (`python3 bench/blaze_ui.py --per-kind 300`)

| kind | sample | blaze | zustc |
|---|---:|---:|---:|
| run-pass | 300 | 254 (84.7%) | 224 (74.7%) |
| check-pass | 300 | 274 (91.3%) | 245 (81.7%) |
| build-pass | 135 | 123 (91.1%) | 97 (71.9%) |
