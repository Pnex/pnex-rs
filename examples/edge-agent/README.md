# PNeX edge agent — ingestion examples

The agent (`pnex-agent`, installed from the PNeX UI: Devices → Register →
*Edge agent*) exposes a **password-less local HTTP API**, on
`http://127.0.0.1:7070` by default. Every accepted point is written to a
durable disk queue before the `202` answer, then forwarded to PNeX over the
encrypted device tunnel — buffering through slow, saturated or broken links.

**No schema**: any key, any JSON value, optional unit. Keys appear in the
UI on first sight. Every value reaches the live cache (flows, live
dashboards); tick *History* on a key in the UI — or send `"record": true` —
to also store it in OpenObserve.

## API

| Method | Path | Body |
|---|---|---|
| `POST` | `/v1/points` | `{"key","value","unit"?,"ts"?,"record"?}`, an array of those, a flat object `{"temp":21.5,"hum":40,"ts"?}`, or `text/plain` lines `key=value` |
| `POST` | `/v1/points/{key}` | raw value (`21.5`, `true`, `"text"`, `{…}`); query `?unit=…&record=true` |
| `GET` | `/v1/status` | link state, queue depth, counters |
| `GET` | `/v1/metrics` | keys seen locally since start |
| `GET` | `/healthz` | `ok` |

- `ts`: epoch **milliseconds** or RFC 3339 string; default = reception time.
- Numbers and booleans become metric series; text and objects become events.
- `202` = durably queued. `400` = unreadable body. `503` + `Retry-After` =
  the disk cannot keep up: retry later (backpressure).
- Max 10 000 points / 8 MiB per request.

## Examples

| Language | File | Dependencies |
|---|---|---|
| shell | [`bash/push.sh`](bash/push.sh) | curl |
| Python | [`python/push.py`](python/push.py) | none (stdlib) |
| Python (high rate) | [`python/bulk_async.py`](python/bulk_async.py) | none (stdlib, threads) |
| TypeScript | [`typescript/push.ts`](typescript/push.ts) | Node ≥ 18 / Deno / Bun |
| PowerShell | [`powershell/push.ps1`](powershell/push.ps1) | none |
| Go | [`go/main.go`](go/main.go) | none (stdlib) |

The CLI also pushes a value: `pnex-agent send temperature 21.5 --unit °C`.
