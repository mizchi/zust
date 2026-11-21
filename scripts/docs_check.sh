#!/usr/bin/env bash
# 英語の文書と日本語訳が食い違っていないかを jev-lint docs で確かめる。
#
#   scripts/docs_check.sh           記録 (.jev-lint/*-baseline.json) を今の文書で読み直す (オフライン、API キー不要。CI)
#   scripts/docs_check.sh --update  文書が記録から変わったマニフェストだけ、モデルに判定させて記録を書き直す
#                                   (TYPESAFE_API_KEY か TYPESAFEAI_API_KEY が要る。コミットフック)
#
# API キーがないとき (TYPESAFE_API_KEY / TYPESAFEAI_API_KEY が空) は、記録が古いマニフェストを判定し直せないので
# 飛ばす (警告を出して失敗にしない)。組がそろっているか・マニフェストが最新か・記録のある組が食い違っていないかは
# キーがなくても確かめる。DOCS_CHECK_STRICT=1 なら飛ばさず、古い記録を失敗 (3) にする。
#
# 終了コード: 0 一致 (キーがなく飛ばしたものを含む)、1 食い違い (どの組かを出す)、3 記録が古い (--update で書き直す)、
# 2 そのほかのエラー
set -uo pipefail
cd "$(dirname "$0")/.."
JEV="npx -y jev-lint@0.8.0"
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
  case "$m" in *-baseline.json) continue ;; esac
  b="${m%.json}-baseline.json"
  if [ -f "$b" ]; then
    $JEV docs --document-manifest "$m" --replay "$b" >/tmp/jev-docs.$$ 2>&1
    rc=$?
  else
    rc=3
  fi
  # (記録が古く、判定し直すキーもない: 飛ばす)
  if [ $rc -eq 3 ] && [ $have_key -eq 0 ] && [ $strict -eq 0 ]; then
    msg="docs: $m の記録が古いが、TYPESAFE_API_KEY がないので判定を飛ばした (キーを設定して scripts/docs_check.sh --update で記録する)"
    echo "$msg" >&2
    [ -n "${GITHUB_ACTIONS:-}" ] && echo "::warning::$msg"
    skipped=$((skipped + 1))
    continue
  fi
  if [ $rc -eq 3 ] && [ $update -eq 1 ]; then
    if [ $have_key -eq 0 ]; then
      echo "docs: $m の記録が古い。TYPESAFE_API_KEY を設定して scripts/docs_check.sh --update を実行する" >&2
      status=3
      continue
    fi
    echo "docs: $m が変わったので判定し直す" >&2
    $JEV docs --document-manifest "$m" --record "$b" >/tmp/jev-docs.$$ 2>&1
    rc=$?
    chmod 644 "$b" 2>/dev/null
  fi
  case $rc in
    0) ;;
    1) echo "docs: 英語と日本語が食い違っている ($m):" >&2; grep -v '^{' /tmp/jev-docs.$$ | head -40 >&2; status=1 ;;
    3) echo "docs: $m の記録が古い (scripts/docs_check.sh --update で書き直す)" >&2; [ $status -eq 0 ] && status=3 ;;
    *) echo "docs: jev-lint が失敗した ($m, 終了コード $rc):" >&2; tail -20 /tmp/jev-docs.$$ >&2; [ $status -eq 0 ] && status=2 ;;
  esac
done
rm -f /tmp/jev-docs.$$
[ $skipped -gt 0 ] && echo "docs: $skipped 個のマニフェストは API キーがないので判定していない" >&2
exit $status
