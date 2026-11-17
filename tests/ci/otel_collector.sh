#!/usr/bin/env bash
# Send treeproof's gen_ai.evaluation.result events to a real OpenTelemetry Collector
# (otelcol-contrib, file exporter) and check what arrives.
set -euo pipefail
ver="${OTELCOL_VERSION:-0.165.0}"
work="$(mktemp -d)"
cd "$work"
curl -fsSL -o oc.tgz "https://github.com/open-telemetry/opentelemetry-collector-releases/releases/download/v${ver}/otelcol-contrib_${ver}_linux_amd64.tar.gz"
tar xzf oc.tgz otelcol-contrib
cat > cfg.yaml <<YAML
receivers:
  otlp:
    protocols:
      http:
        endpoint: 127.0.0.1:4318
exporters:
  file:
    path: $work/out.json
service:
  pipelines:
    logs:
      receivers: [otlp]
      exporters: [file]
YAML
./otelcol-contrib --config cfg.yaml > col.log 2>&1 &
col=$!
trap 'kill $col 2>/dev/null || true' EXIT
for _ in $(seq 1 30); do curl -s -o /dev/null -X POST -H 'Content-Type: application/json' -d '{}' http://127.0.0.1:4318/v1/logs && break; sleep 1; done

export TREEPROOF_CACHE_DIR="$work/cache" TREEPROOF_KEY_FILE="$work/key"
mkdir repo && cd repo && git init -q -b main
cp "$GITHUB_WORKSPACE/tests/fixtures/run_tests.py" .
treeproof run -- python3 run_tests.py >/dev/null
TRACEPARENT=00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01 treeproof otel --endpoint http://127.0.0.1:4318/v1/logs
sleep 3
python3 - "$work/out.json" <<'PY'
import json, sys
seen = {}
for line in open(sys.argv[1]):
    for rl in json.loads(line)["resourceLogs"]:
        for sl in rl["scopeLogs"]:
            for r in sl["logRecords"]:
                a = {x["key"]: list(x["value"].values())[0] for x in r["attributes"]}
                assert r.get("eventName") == "gen_ai.evaluation.result", r
                assert r.get("traceId") == "4bf92f3577b34da6a3ce929d0e0e4736", r
                seen[a["gen_ai.evaluation.name"]] = a["gen_ai.evaluation.score.label"]
want = {"verification.exit_code": "verified", "verification.fs_diff": "verified", "verification.test_rerun": "unknown"}
assert seen == want, seen
print("collector received", seen)
PY
