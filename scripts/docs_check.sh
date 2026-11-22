#!/usr/bin/env bash
# 英語の文書と日本語訳が食い違っていないかを jev-lint docs で確かめる。
#
#   scripts/docs_check.sh           記録 (台帳 .jev-lint/judgments/) から今の文書の判定を読む (オフライン、API キー不要。CI)
#   scripts/docs_check.sh --update  台帳に答えのない比較 (文書が変わった組) だけ、モデルに判定させて台帳に足す
#                                   (TYPESAFE_API_KEY か TYPESAFEAI_API_KEY が要る。コミットフック)
#
# 台帳は jev-lint 0.10 の judgment ledger: 追記だけのファイルのディレクトリで、答えは比較の内容 (抜粋の中身、問い、
# モデル) で引く。全マニフェストで 1 つを共有する。ブランチ同士はファイルの和集合としてそのままマージできる。
#
# API キーがないとき (TYPESAFE_API_KEY / TYPESAFEAI_API_KEY が空) は、答えのない比較があるマニフェストを判定できないので
# 飛ばす (警告を出して失敗にしない)。組がそろっているか・マニフェストが最新か・記録のある組が食い違っていないかは
# キーがなくても確かめる。DOCS_CHECK_STRICT=1 なら飛ばさず、古い記録を失敗 (3) にする。
#
# 終了コード: 0 一致 (キーがなく飛ばしたものを含む)、1 食い違い (どの組かを出す)、3 答えのない比較がある (--update で足す)、
# 2 そのほかのエラー
set -uo pipefail
cd "$(dirname "$0")/.."
JEV="npx -y jev-lint@0.10.0"
LEDGER=.jev-lint/judgments
update=0
[ "${1:-}" = "--update" ] && update=1
have_key=0
[ -n "${TYPESAFE_API_KEY:-}${TYPESAFEAI_API_KEY:-}" ] && have_key=1
strict=0
[ -n "${DOCS_CHECK_STRICT:-}" ] && strict=1
skipped=0

python3 scripts/docs_manifest.py --check || exit 1

status=0
for m in .jev-lint/documents*.json; do
  $JEV docs --document-manifest "$m" --ledger "$LEDGER" --replay >/tmp/jev-docs.$$ 2>&1
  rc=$?
  # (3 は台帳に答えのない比較があるとき (「a/b comparisons answered」で a < b) と、答えが insufficient_context など
  # で決まらないとき。--update で判定するのは前者だけ)
  stale=0
  [ $rc -eq 3 ] && python3 -c 'import re,sys; m=re.search(r"(\d+)/(\d+) comparisons answered", open(sys.argv[1]).read()); sys.exit(0 if m and int(m[1]) < int(m[2]) else 1)' /tmp/jev-docs.$$ && stale=1
  # (答えのない比較があり、判定するキーもない: 飛ばす)
  if [ $rc -eq 3 ] && [ $stale -eq 1 ] && [ $have_key -eq 0 ] && [ $strict -eq 0 ]; then
    msg="docs: $m に台帳の答えがない比較があるが、TYPESAFE_API_KEY がないので判定を飛ばした (キーを設定して scripts/docs_check.sh --update で記録する)"
    echo "$msg" >&2
    [ -n "${GITHUB_ACTIONS:-}" ] && echo "::warning::$msg"
    skipped=$((skipped + 1))
    continue
  fi
  if [ $rc -eq 3 ] && [ $stale -eq 1 ] && [ $update -eq 1 ]; then
    if [ $have_key -eq 0 ]; then
      echo "docs: $m に台帳の答えがない比較がある。TYPESAFE_API_KEY を設定して scripts/docs_check.sh --update を実行する" >&2
      status=3
      continue
    fi
    echo "docs: $m の答えのない比較を判定する" >&2
    $JEV docs --document-manifest "$m" --ledger "$LEDGER" >/tmp/jev-docs.$$ 2>&1
    rc=$?
    stale=0
  fi
  case $rc in
    0) ;;
    1) echo "docs: 英語と日本語が食い違っている ($m):" >&2; grep -v '^{' /tmp/jev-docs.$$ | head -40 >&2; status=1 ;;
    3) if [ $stale -eq 1 ]; then
         echo "docs: $m に台帳の答えがない比較がある (scripts/docs_check.sh --update で足す)" >&2
       else
         echo "docs: $m の判定が決まっていない (insufficient_context など):" >&2; grep -v '^{' /tmp/jev-docs.$$ | head -20 >&2
       fi
       [ $status -eq 0 ] && status=3 ;;
    *) echo "docs: jev-lint が失敗した ($m, 終了コード $rc):" >&2; tail -20 /tmp/jev-docs.$$ >&2; [ $status -eq 0 ] && status=2 ;;
  esac
done
rm -f /tmp/jev-docs.$$
[ $skipped -gt 0 ] && echo "docs: $skipped 個のマニフェストは API キーがないので判定していない" >&2
exit $status
