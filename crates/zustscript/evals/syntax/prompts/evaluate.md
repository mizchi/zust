# 評価の指示(subagent に渡す)

あなたは zustscript(TS 風の見た目で、JS と wasm を出力する言語の試作)の**文法の提案**を評価する。立場は次のとおり。

{{PERSONA}}

## 読むもの(この順に)

1. `crates/zustscript/evals/syntax/README.md` の「評価の前提」
2. `crates/zustscript/evals/syntax/rubric.md`(観点と点の基準)
3. 今の文法: `crates/zustscript/README.md` の「文法」の節。必要なら `crates/zustscript/examples/*.zsc`
4. 評価する提案: {{PROPOSALS}}
5. 提案が書き換えている `crates/zustscript/evals/syntax/corpus/*.zsc`
6. (implementer のみ)関わる実装: `crates/zustscript/src/parse.rs`、`check.rs`、`js.rs`、`lower.rs`

ファイルは読むだけにする(何も書き換えない)。

## やること

提案ごとに、ルーブリックの R1〜R8 を 1〜5 点で採点し、根拠を書く。

- 根拠は具体的に書く: 提案の中のどのコード、どの規則を見て、そう判断したか
- 立場に沿って評価する: その立場の人がこの文法で実際に書くとき、読むとき、何に引っかかるか
- 褒めるより、問題を見つけることを優先する。ただし、問題でないものを問題にしない
- 提案が答えていない問いは questions に入れる
- 提案より良い書き方を思いついたら、alternative にコード例つきで書く

## 出力

最後に、次の JSON だけを ```json のコードブロックで出す(提案が複数なら、配列の要素を提案ごとに)。文章の部分は日本語で書く。

```json
[
  {
    "proposal": "001-variants",
    "persona": "ts-dev",
    "scores": {
      "R1": { "score": 4, "why": "..." },
      "R2": { "score": 3, "why": "..." },
      "R3": { "score": 4, "why": "..." },
      "R4": { "score": 4, "why": "..." },
      "R5": { "score": 3, "why": "..." },
      "R6": { "score": 3, "why": "..." },
      "R7": { "score": 4, "why": "..." },
      "R8": { "score": 4, "why": "..." }
    },
    "blocker": [],
    "issues": [
      { "severity": "high", "where": "提案の節 / コードの行", "problem": "...", "suggestion": "..." }
    ],
    "questions": ["..."],
    "alternative": { "summary": "...", "example": "..." }
  }
]
```
