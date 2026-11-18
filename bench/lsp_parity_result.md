#### synth (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | zustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 100%) | +0.0 点 |
| hover (名前を含む) | 100% | 100% | +0.0 点 |
| references (数の一致) | 100% | 100% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 100% (断ったのも同じ 0/0) | 同じ 100% (断ったのも同じ 0/0) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 100%) | 同じ 100% (答えた 95%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 10) | 100% (候補 3) | +0.0 点 |
| completion `na\|me` (同) | 97% (候補 25) | 91% (候補 145) | +0.0 点 |
| 一致の平均 | 99.7% | 99.1% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.2/0.4, hove 0.2/0.5, comp 0.6/0.7, refe 0.4/0.9, rena 0.4/1.1, sign 0.3/0.4, docu 0.5/0.9 | defi 0.2/0.5, hove 0.3/0.5, comp 2.0/4.0, refe 0.6/4.1, rena 0.6/3.1, sign 0.2/0.5, docu 0.4/0.6 | defi -0.2, hove -0.1, comp -0.2, refe -0.2, rena -0.1, sign -0.1, docu -0.1 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 9 ms (511 ms) / 20 MB | 2242 ms (3096 ms) / 864 MB | -1 ms / +0 MB |

#### fixture (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | zustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 90%) | 同じ 100% (答えた 90%) | +0.0 点 |
| hover (名前を含む) | 88% | 88% | +0.0 点 |
| references (数の一致) | 100% | 100% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 96% (断ったのも同じ 14/14) | 同じ 100% (断ったのも同じ 14/14) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 50%) | 同じ 100% (答えた 45%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 26) | 100% (候補 18) | +0.0 点 |
| completion `na\|me` (同) | 90% (候補 3) | 69% (候補 76) | +0.0 点 |
| 一致の平均 | 97.3% | 95.6% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.2/0.4, hove 0.2/0.5, comp 0.5/1.3, refe 0.3/0.9, rena 0.3/0.8, sign 0.2/0.4, docu 0.3/0.8 | defi 0.2/0.6, hove 0.3/2.4, comp 2.9/7.3, refe 0.5/3.9, rena 0.3/0.4, sign 0.2/0.3, docu 0.3/1.1 | defi -0.2, hove -0.2, comp -0.4, refe -0.2, rena -0.1, sign -0.0, docu -0.1 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 7 ms (508 ms) / 18 MB | 336 ms (3195 ms) / 855 MB | -3 ms / -1 MB |

#### zustc (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | zustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 78%) | 同じ 100% (答えた 78%) | +0.0 点 |
| hover (名前を含む) | 88% | 88% | +0.0 点 |
| references (数の一致) | 97% | 100% | +0.0 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 94% (断ったのも同じ 23/23) | 同じ 100% (断ったのも同じ 23/23) | +0.0 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 95%) | 同じ 100% (答えた 95%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 33) | 100% (候補 26) | +0.0 点 |
| completion `na\|me` (同) | 84% (候補 9) | 68% (候補 85) | +0.0 点 |
| 一致の平均 | 96.3% | 95.5% | +0.0 点 |
| 時間 p50/p90 (ms) | defi 0.5/1.2, hove 0.4/0.8, comp 1.1/2.6, refe 2.9/32.8, rena 0.5/3.1, sign 0.9/2.5, docu 0.8/2.1 | defi 0.5/1.5, hove 0.6/1.5, comp 6.3/21.4, refe 14.2/880.1, rena 0.6/39.8, sign 0.4/2.3, docu 1.4/4.0 | defi -0.4, hove -0.5, comp -0.6, refe -1.3, rena -0.4, sign -0.6, docu -0.3 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 104 ms (505 ms) / 66 MB | 4384 ms (5990 ms) / 2190 MB | -26 ms / +0 MB |

#### ide (40 箇所、`.` の後 20 箇所。比べる相手は rust-analyzer)

| 機能 | zustc | rust-analyzer | zustc の前回との差 |
|---|---|---|---|
| 対応を返す機能 | docsym, wsym, definition, hover, completion, references, rename, signature, highlight | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, highlight, inlay, codeAction, format | |
| documentSymbol (名前の一致) | 名前 100% (種類も 100%) | 名前 100% (種類も 100%) | +0.0 点 |
| workspace/symbol (上位 20 の一致) | 100% | 100% | +0.0 点 |
| definition (行き先の一致) | 同じ 100% (答えた 90%) | 同じ 100% (答えた 90%) | +0.0 点 |
| hover (名前を含む) | 95% | 90% | -2.5 点 |
| references (数の一致) | 100% | 100% | +2.5 点 |
| documentHighlight (数の一致) | 100% | 100% | +0.0 点 |
| rename (書き換えの一致) | 同じ 100% (断ったのも同じ 13/13) | 同じ 100% (断ったのも同じ 13/13) | +3.7 点 |
| signatureHelp (名前・引数の数・今の引数の一致) | 同じ 100% (答えた 95%) | 同じ 100% (答えた 95%) | +0.0 点 |
| completion `x.\|` (書いてある名前を含む) | 100% (候補 72) | 100% (候補 72) | +0.0 点 |
| completion `na\|me` (同) | 95% (候補 11) | 85% (候補 193) | +5.5 点 |
| 一致の平均 | 99.0% | 97.5% | +0.9 点 |
| 時間 p50/p90 (ms) | defi 0.9/1.8, hove 0.9/1.6, comp 1.8/3.8, refe 4.6/19.1, rena 2.1/10.7, sign 1.5/2.9, docu 2.5/11.4 | defi 0.6/2.2, hove 0.5/1.2, comp 8.4/14.8, refe 10.5/1066.4, rena 1.0/12.7, sign 0.3/0.4, docu 1.6/8.8 | defi -0.3, hove -0.1, comp +0.1, refe -1.9, rena -1.5, sign +0.2, docu -1.9 |
| 起動から名前が引けるまで (作業が終わるまで) / メモリの最大 (RSS) | 203 ms (506 ms) / 67 MB | 5166 ms (5517 ms) / 1778 MB | +58 ms / +0 MB |
