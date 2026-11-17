#!/usr/bin/env bash
# Verify treeproof receipts for the treeproof/verify action.
set -uo pipefail
shopt -s nullglob

args=(verify --no-local-key --json)
if [ "${TP_CURRENT}" = "true" ]; then args+=(--tree current); fi
if [ "${TP_RERUN}" = "true" ]; then args+=(--rerun); fi
while IFS= read -r k; do
  k="${k#"${k%%[![:space:]]*}"}"; k="${k%"${k##*[![:space:]]}"}"
  [ -n "$k" ] && args+=(--key "$k")
done <<< "${TP_KEYS:-}"

# Word-split the glob on purpose so it expands.
# shellcheck disable=SC2206
files=( ${TP_RECEIPTS} )
report="${RUNNER_TEMP:-/tmp}/treeproof-report.json"
echo "report=$report" >> "$GITHUB_OUTPUT"

if [ ${#files[@]} -eq 0 ]; then
  echo "::error title=treeproof::no receipt files match ${TP_RECEIPTS}. Run \`treeproof export\` after a passing \`treeproof run\` and commit .treeproof/."
  echo "ok=false" >> "$GITHUB_OUTPUT"
  exit 1
fi

treeproof "${args[@]}" "${files[@]}" > "$report"
status=$?
cat "$report"

if [ $status -eq 0 ]; then
  echo "ok=true" >> "$GITHUB_OUTPUT"
  {
    echo "### treeproof: verified"
    echo
    echo "The committed receipt for this tree verified and its command re-ran with the same result."
  } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
  exit 0
fi

echo "ok=false" >> "$GITHUB_OUTPUT"
python3 "$GITHUB_ACTION_PATH/annotate.py" "$report" 2>/dev/null || python "$GITHUB_ACTION_PATH/annotate.py" "$report" 2>/dev/null || true
{
  echo "### treeproof: verification failed"
  echo
  echo "See the job log for the receipt that did not verify or did not reproduce."
} >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit 1
