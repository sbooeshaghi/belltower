# Live OTLP Consumer Acceptance Log — 2026-07-29

## Outcome

**PASS on the integrated worktree, not yet immutable release evidence.**
Belltower's existing ignored live-push test projected a two-turn canonical
session into OTLP protobuf, posted it to a genuine OpenTelemetry Collector
receiver, and received a successful OTLP response. The collector both decoded
the payload through its debug exporter and persisted the ingested trace data
through its file exporter for independent inspection.

This closes the behavioral question that a maintained real OTLP consumer can
ingest and expose Belltower's current projection. It does **not** replace the
release requirement to repeat this lane on the clean, unchanged commit that
receives the tag. It also does not claim Phoenix-specific UI behavior; Phoenix
was preferred, but its local installation exceeded the bounded setup window,
so the documented release-plan fallback of another genuine OTLP consumer was
used.

## Source and environment

- repository: `belltower`
- branch: `codex/foundation-integrity`
- observed `HEAD`: `478c5cfea8b925af0fc5fb6dbe96a3b6e95fc064`
- worktree: dirty integrated review tree (`106` porcelain entries immediately
  before this log was added)
- host: macOS 26.3.1 (25D771280a), Darwin amd64
- Rust: `rustc 1.93.0 (254b59607 2026-01-19)`
- Cargo: `cargo 1.93.0 (083ac5135 2025-12-15)`
- consumer: official `otelcol-contrib 0.157.0`
- collector release: <https://github.com/open-telemetry/opentelemetry-collector-releases/releases/tag/v0.157.0>
- downloaded archive SHA-256:
  `e11e7482144c3ac1eb1f612d3d175589435cad968a791d6ef5c73be43e1b8c34`

The archive digest matched the corresponding entry in the official v0.157.0
checksums manifest before execution.

## Preferred-consumer attempt and bounded fallback

No Phoenix, Collector, Docker, or Podman installation was present, and no
Phoenix/Arize endpoint credentials were available. A temporary Python 3.12
environment was created and the current `arize-phoenix` package was resolved
(`19.10.0`, 148-package environment). Download/extraction did not finish within
the declared 14-minute setup cutoff, so that attempt was terminated without
starting Phoenix. No product or test failure occurred in that attempt.

The fallback downloaded the official OpenTelemetry Collector Contrib v0.157.0
Darwin amd64 release and its official checksum manifest:

```bash
mkdir -p /tmp/belltower-otlp-gate-20260729
curl -fsSL \
  https://github.com/open-telemetry/opentelemetry-collector-releases/releases/download/v0.157.0/otelcol-contrib_0.157.0_darwin_amd64.tar.gz \
  -o /tmp/belltower-otlp-gate-20260729/otelcol-contrib.tar.gz
curl -fsSL \
  https://github.com/open-telemetry/opentelemetry-collector-releases/releases/download/v0.157.0/otelcol-contrib_0.157.0_checksums.txt \
  -o /tmp/belltower-otlp-gate-20260729/checksums.txt
shasum -a 256 /tmp/belltower-otlp-gate-20260729/otelcol-contrib.tar.gz
rg 'otelcol-contrib_0.157.0_darwin_amd64.tar.gz' \
  /tmp/belltower-otlp-gate-20260729/checksums.txt
tar -xzf /tmp/belltower-otlp-gate-20260729/otelcol-contrib.tar.gz \
  -C /tmp/belltower-otlp-gate-20260729 otelcol-contrib
/tmp/belltower-otlp-gate-20260729/otelcol-contrib --version
```

The collector ran only on loopback. Internal metrics were disabled to avoid an
unrelated listener; the health endpoint and OTLP/HTTP receiver were
`127.0.0.1:13133` and `127.0.0.1:14318` respectively. Both a detailed debug
exporter and a file exporter consumed the receiver pipeline:

```yaml
extensions:
  health_check:
    endpoint: 127.0.0.1:13133

receivers:
  otlp:
    protocols:
      http:
        endpoint: 127.0.0.1:14318

exporters:
  debug:
    verbosity: detailed
  file:
    path: /tmp/belltower-otlp-gate-20260729/ingested-traces.jsonl

service:
  extensions: [health_check]
  telemetry:
    metrics:
      level: none
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [debug, file]
```

The configuration and consumer startup were checked with:

```bash
/tmp/belltower-otlp-gate-20260729/otelcol-contrib validate \
  --config /tmp/belltower-otlp-gate-20260729/collector.yaml
/tmp/belltower-otlp-gate-20260729/otelcol-contrib \
  --config /tmp/belltower-otlp-gate-20260729/collector.yaml
curl -fsS http://127.0.0.1:13133/
```

The health response was `{"status":"Server available",...}` and the collector
reported `Everything is ready. Begin running and processing data.`

## Belltower push

From the repository root, the unchanged ignored live-consumer test was run
against the loopback receiver:

```bash
PHOENIX_COLLECTOR_ENDPOINT=http://127.0.0.1:14318 \
PHOENIX_API_KEY=local-otlp-gate \
ARIZE_SPACE_ID=local-otlp-gate \
cargo test -p bt-server \
  tests::live_otlp_push_to_arize_preserves_session_correlation_across_turn_traces \
  --locked -- --ignored --exact --nocapture
```

The API key and space values were non-secret placeholders required by the
test's environment contract; the loopback Collector did not authenticate the
request. Belltower normalized the configured collector endpoint to
`http://127.0.0.1:14318/v1/traces` and sent OTLP protobuf.

Observed result:

```text
live arize validation ok session_id=e9d1ef70-5bad-4c4a-894b-abff6a796720 request_url=http://127.0.0.1:14318/v1/traces bytes_sent=38342
test tests::live_otlp_push_to_arize_preserves_session_correlation_across_turn_traces ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 95 filtered out
```

## Independent consumer-side assertions

The collector's persisted output was parsed independently after the successful
OTLP response. The following assertions held:

- one decoded OTLP document containing one resource-span group
- 8 spans total: 2 `turn`, 4 `llm_call`, and 2 `tool_execution`
- 2 unique trace IDs, one per turn
- OpenInference kinds: 2 `AGENT`, 4 `LLM`, and 2 `TOOL`
- all 8 spans carried the same `session.id`,
  `e9d1ef70-5bad-4c4a-894b-abff6a796720`
- the 6 non-root spans carried parent-span links to their turn roots
- `service.name=belltower`
- `openinference.project.name=belltower-live-turn-traces`
- every span exposed `input.value` and `output.value`
- every LLM span exposed `llm.token_count.prompt`,
  `llm.token_count.completion`, and `llm.token_count.total`
- the detailed consumer output reported 1 resource-span group and 8 spans,
  with status `Ok` and zero dropped attributes, events, or links on the
  inspected spans

The machine-readable inspection command was:

```bash
python3 -c 'import json,collections,pathlib; p=pathlib.Path("/tmp/belltower-otlp-gate-20260729/ingested-traces.jsonl"); docs=[json.loads(x) for x in p.read_text().splitlines() if x.strip()]; spans=[s for d in docs for rs in d["resourceSpans"] for ss in rs["scopeSpans"] for s in ss["spans"]]; attrs=lambda xs:{a["key"]:next(iter(a["value"].values())) for a in xs}; resources=[attrs(rs["resource"]["attributes"]) for d in docs for rs in d["resourceSpans"]]; sas=[attrs(s.get("attributes",[])) for s in spans]; print("documents",len(docs)); print("spans",len(spans)); print("names",dict(sorted(collections.Counter(s["name"] for s in spans).items()))); print("oi_kinds",dict(sorted(collections.Counter(a.get("openinference.span.kind") for a in sas).items()))); print("trace_ids",sorted(set(s["traceId"] for s in spans))); print("session_ids",sorted(set(a.get("session.id") for a in sas))); print("service_names",sorted(set(r.get("service.name") for r in resources))); print("projects",sorted(set(r.get("openinference.project.name") for r in resources))); print("all_have_session_id",all(a.get("session.id") for a in sas)); print("all_have_input_output",all("input.value" in a and "output.value" in a for a in sas)); llm=[a for s,a in zip(spans,sas) if s["name"]=="llm_call"]; print("llm_token_attrs_complete",all(all(k in a for k in ("llm.token_count.prompt","llm.token_count.completion","llm.token_count.total")) for a in llm)); print("parent_links",sum(bool(s.get("parentSpanId")) for s in spans));'
```

Its output was:

```text
documents 1
spans 8
names {'llm_call': 4, 'tool_execution': 2, 'turn': 2}
oi_kinds {'AGENT': 2, 'LLM': 4, 'TOOL': 2}
trace_ids ['758adf394a2a498fbe570918915098a8', 'e68fc2f009894e6a9d14de6198fedfd7']
session_ids ['e9d1ef70-5bad-4c4a-894b-abff6a796720']
service_names ['belltower']
projects ['belltower-live-turn-traces']
all_have_session_id True
all_have_input_output True
llm_token_attrs_complete True
parent_links 6
```

The collector was then interrupted and reported `Shutdown complete.` Temporary
collector, output, Phoenix-environment, and installer-cache roots used by this
lane were removed after this record was written. The shared global uv cache was
not altered because it is not exclusive to this acceptance run.

## Release interpretation

This lane validates the intended architecture: the durable canonical session
log remains the source of truth, while OTLP/OpenInference is a derived,
consumer-compatible projection. It demonstrates cross-turn session
correlation, root/child structure, LLM/tool semantics, inputs/outputs, token
counts, and project/resource identity after real ingestion.

For the final release decision, rerun this exact lane (or the preferred Phoenix
lane) from the clean candidate SHA, record the exact unchanged SHA and consumer
version, and retain evidence that the consumer exposed the expected two traces
and eight correlated spans. This worktree run must not be cited as immutable
tag evidence.
