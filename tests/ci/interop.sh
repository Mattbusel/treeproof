#!/usr/bin/env bash
# Produce a real receipt and verify it with securesystemslib and in-toto-attestation.
set -euo pipefail
work="$(mktemp -d)"
export TREEPROOF_CACHE_DIR="$work/cache" TREEPROOF_KEY_FILE="$work/key"
mkdir "$work/repo" && cd "$work/repo" && git init -q -b main
cp "$GITHUB_WORKSPACE/tests/fixtures/run_tests.py" .
treeproof run -- python3 run_tests.py >/dev/null
treeproof export >/dev/null
treeproof key export > "$work/pub.pem"
python -I "$GITHUB_WORKSPACE/tests/interop/check_receipt.py" .treeproof/receipts/*.json "$work/pub.pem" "$(treeproof tree)"
