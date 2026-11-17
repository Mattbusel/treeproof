# OpenTelemetry evaluation events

treeproof can report what it verified as OpenTelemetry [`gen_ai.evaluation.result`](https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/gen-ai-events.md) events, so an agent's "done" can be scored next to its traces in any OTLP backend.

## Events

One log record per check, with event name `gen_ai.evaluation.result`:

| `gen_ai.evaluation.name` | `verified` when | `contradicted` when | `unknown` when |
|---|---|---|---|
| `verification.exit_code` | a signed receipt shows the command passed | the receipt for the tree is a failure | no run was recorded |
| `verification.fs_diff` | the receipt's tree equals the current working tree | the tree changed since the run | there is nothing to compare |
| `verification.test_rerun` | a re-run agrees with the receipt | a re-run disagrees | no re-run was done (the default) |

Attributes: `gen_ai.evaluation.name`, `gen_ai.evaluation.score.label`, `gen_ai.evaluation.score.value` (1.0 verified, 0.0 contradicted, absent for unknown), `gen_ai.evaluation.explanation`, and `vcs.ref.head.revision` (the tree id). When `TRACEPARENT` holds a W3C trace context, each record carries its `traceId` and `spanId`, which attaches it to the host's span.

## Sending

```sh
treeproof otel --endpoint http://localhost:4318/v1/logs            # newest receipt for the current tree
treeproof otel --rerun receipt.json                                 # include a re-run
treeproof otel --dry-run                                            # print the OTLP JSON
```

The `Stop` hook also sends `exit_code`, `fs_diff` and `test_rerun` (as `unknown`) each time it evaluates the gate, when an endpoint is configured.

Endpoint resolution: `--endpoint`, then `TREEPROOF_OTLP_ENDPOINT`, then `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`, then `OTEL_EXPORTER_OTLP_ENDPOINT` + `/v1/logs`. `OTEL_EXPORTER_OTLP_HEADERS` is honoured. `TREEPROOF_OTEL=0` turns export off.

Transport is OTLP/HTTP with JSON encoding over plain HTTP, meant for a local collector. treeproof does not speak gRPC or TLS; when `OTEL_EXPORTER_OTLP_PROTOCOL` is `grpc` the hook does not export. Run an OpenTelemetry Collector next to the agent to forward to a remote backend.

## Tested against

CI starts `otelcol-contrib` with an OTLP/HTTP receiver and a file exporter, sends a receipt's events, and checks the event name, trace id, evaluation names and labels in what the collector wrote (`tests/ci/otel_collector.sh`).

Whether a given backend renders `gen_ai.evaluation.result` events in its UI is up to that backend; treeproof emits the semantic-convention shape and nothing vendor specific.
