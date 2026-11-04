# Discovery: sonda

Read on 2026-11-04 by a discovery session (read-only) from the copy under `reference/` (Sonda 1.29.0) and the workshops code. Facts come from reading the documentation and the code; nothing was run.

**Discovery report: what Sonda offers a terminal program, and what the workshops already do with it**

Rules: `netpanda-ai-workbench rules, version 0.12.0, file workbench-00-about.md`

- All the always-on rule files carry the same version, 0.12.0, so the versions agree.
- I can see all ten sections: About these rules; Approval boundaries; Think before acting; Keep changes small; Verify, then report what happened; Delegating work; Review standards; Writing standard; Code comments; Tooling.
- `.claude/profiles/software.md` is visible and also says 0.12.0.
- I read files only and ran no command.
- Docs version: `reference/sonda/Cargo.toml` says workspace `version = "1.29.0"`. The docs pages carry no version of their own.

## Known facts

### 1. How Sonda runs

- There are two programs from one workspace: `sonda` (CLI) and `sonda-server` (HTTP). The crates are `sonda-core`, `sonda`, `sonda-server` and `sonda-wasm`. (`reference/sonda/Cargo.toml`)
- The CLI has six verbs: `run`, `list`, `show`, `new`, `test`, `completions`. (`reference/sonda/docs/site/docs/reference/cli-flags.md`)
  - `run` runs a file or `@name`, with overrides `--rate`, `--duration`, `--sink`, `--endpoint`, `--encoder`, `-o`, `--label` and `--on-sink-error`.
  - `list` takes `--kind`, `--tag` and `--json`.
  - Global flags are `--catalog <DIR>`, `-q`, `-v`, `--dry-run` and `--format text|json`.
  - Exit codes are 0, 1 (runtime error) and 2 (parse error).
  - `sonda --dry-run --format json run <file>` validates and prints the resolved config as JSON.
  - Per-signal subcommands and the `SONDA_SCENARIO_PATH` and `SONDA_PACK_PATH` variables were removed in 1.9.
- The server flags are (`reference/sonda/docs/site/docs/deploy/server.md`):

  | Flag | Env var | Default |
  |---|---|---|
  | `--port` | none | 8080 |
  | `--bind` | none | 0.0.0.0 |
  | `--api-key` | `SONDA_API_KEY` | unset (no auth) |
  | `--catalog <DIR>` | `SONDA_CATALOG` | unset |
  | `--autostart` | `SONDA_AUTOSTART` | false |
  | `--workers` | none | min(cores, 16) |
  | `--max-scenarios` | none | 0 (unlimited) |
  | `--max-inflight-requests` | none | 4 × workers |
  | `--request-timeout` | none | 30 s |
  | `--max-body-bytes` | none | 1 MB |

  - `RUST_LOG` sets the log level.
  - `--autostart` needs `--catalog`.
- Authentication is a bearer token: `Authorization: Bearer <key>`. (`reference/sonda/docs/site/docs/deploy/http-api.md`)
  - `/health` and `/ready` are always public.
  - Everything else needs the key when one is set, including `/metrics`, `/events`, `/gnmi/targets` and `/snmp/agents`.
  - A wrong or missing key gives 401 with `{"error":"unauthorized","detail":...}`.
- The Docker image is `ghcr.io/davidban77/sonda`. (`reference/sonda/docs/site/docs/deploy/docker.md`)
  - Its entrypoint is `sonda-server` on port 8080.
  - It is built `FROM scratch`, so there is no shell, `curl` or `wget`.
  - Do not add a Compose `healthcheck:`; poll from the host instead.
- The only version signal over HTTP is the `sonda_server_build_info{version,git_sha}` series on `GET /metrics`. (`reference/sonda/docs/site/docs/deploy/server-metrics.md`) I found no `/version` endpoint.

### 2. Every HTTP endpoint

Source: `reference/sonda/docs/site/docs/deploy/http-api.md` and `reference/sonda/docs/site/docs/deploy/server-metrics.md`.

| Method and path | Auth | Input | Output |
|---|---|---|---|
| `GET /health` | no | none | `200 {"status":"ok"}`. It reads no server state. |
| `GET /ready` | no | none | `{status, autostart_expected, autostart_started}`. `status` is `not_configured` (200), `in_progress` (503), `finished` (200) or `failed` (503, final until restart). |
| `POST /scenarios` | yes | Body is YAML (`text/yaml`, `application/x-yaml`) or JSON. Optional `?validate=strict`. | `201`. One entry gives `{id,name,state}`; two or more give `{scenarios:[...]}`. A `warnings` array appears only when there is a warning, such as a loopback sink URL. |
| `GET /scenarios` | yes | none | `{scenarios:[{id,name,state,elapsed_secs,degraded}]}` |
| `GET /scenarios/{id}` | yes | none | Config, stats, elapsed time. Adds `pending_ref` only when the state is `unresolved`. |
| `DELETE /scenarios/{id}` | yes | none | `200 {id,status:"stopped",total_events}`. A second DELETE gives 404. |
| `GET /scenarios/{id}/stats` | yes | none | Counters, listed below. |
| `GET /scenarios/{id}/metrics` | yes | none | Prometheus text for one scenario. |
| `GET /scenarios/metrics` | yes | `?label=k:v` (repeatable, AND), `?include_state=a,b` | Prometheus text, one sample per series, no timestamps. An empty body is still 200. |
| `GET /metrics` | yes | none | Server process metrics: 12 `sonda_server_*` series. |
| `POST /events` | yes | JSON only: `{signal_type: logs\|metrics, labels, log\|metric, encoder, sink}` | `200 {sent,signal_type,latency_ms}`. It blocks until the sink acknowledges. |
| `GET /gnmi/targets` | yes | none | The gNMI listeners. Present only in builds with the `gnmi` feature; otherwise 404. |
| `GET /snmp/agents` | yes | none | The SNMP agents. Present only in builds with the `snmp` feature; otherwise 404. |

- **`/stats` fields:**
  - Counters: `total_events`, `current_rate`, `target_rate`, `bytes_emitted`, `errors`, `uptime_secs`.
  - State: `state`, `current_state_secs`, `in_gap`, `in_burst`, `cumulative_resolution_attempts`.
  - Sink health: `consecutive_failures`, `total_sink_failures`, `last_sink_error`, `last_successful_write_at` (Unix nanoseconds), `degraded`.
- **Six lifecycle states:** `pending`, `running`, `paused`, `held`, `unresolved`, `finished`.
  - `pending` can go straight to `paused`.
  - `finished` rows keep occupying a slot until a DELETE.
- **`degraded`** means there were sink failures and no successful delivery in the last 30 s.
- **Error codes:**
  - 400: bad body, missing `version: 2`, or v1 body.
  - 401: bad or missing key.
  - 404: unknown id.
  - 409: the posted `scenario_name` matches an active one; the body lists `conflicting_scenarios`. There is no force flag.
  - 422: runtime validation failed; with `?validate=strict`, `{error:"unresolved_refs",...}`.
  - 429: `capacity_exceeded`, with a `by_state` breakdown.
  - 502: sink error.
  - 408 and 413 carry no JSON body.
- **Not documented:** I found no pause or resume endpoint, no update-in-place, no streaming (SSE or WebSocket), no log-tail endpoint, no pagination or filter on `GET /scenarios`, and no catalog-listing endpoint. `POST /events` has no burst mode and handles only `logs` and `metrics`. There is no `sonda emit` CLI.
- **Stability:** no page marks any endpoint stable, experimental or deprecated. The only "stable contract" wording is for `sonda list --json`.

### 3. Scenario file format

Sources: `reference/sonda/docs/site/docs/build/scenario-files.md`, `reference/sonda/docs/site/docs/reference/scenario-fields.md`, `reference/sonda/docs/site/docs/build/catalogs-and-packs.md`.

- The top level is `version: 2` and `kind: runnable|composable`. Catalog metadata is optional: `scenario_name`, `tags`, `description`.
- Next come `defaults:` (`rate`, `duration`, `encoder`, `sink`, `labels`, `on_sink_error`) and `scenarios:`. Each entry can override a default.
- Entry fields:
  - Core: `id`, `signal_type`, `name`, `rate`, `duration`, `generator` or `log_generator`, `encoder`, `sink`, `labels`, `dynamic_labels`, `jitter`, `metric_type`, `help`, `on_sink_error`, `start_time`.
  - Scheduling: `phase_offset`, `clock_group`, gap, burst and cardinality-spike windows.
  - Dependencies: `after`, `while`, `delay`.
  - Packs: `pack`, `overrides`.
- Unknown fields are errors. A duration needs a unit (`10s`, not `10`).
- `${VAR}` and `${VAR:-default}` are substituted from the environment of the process that runs the scenario.
- `after:` is a one-shot trigger. The entry stays `pending` until the upstream value crosses a threshold. The compiler computes a `phase_offset` and auto-assigns a `chain_<head>` clock group.
  - It needs a `duration`.
  - Supported targets: `flap`, `saturation`, `leak`, `degradation`, `spike_event`.
- `while:` is continuous gating with `ref`, `op` (only `<` or `>`) and `value`.
  - `delay: {open, close}` debounces it.
  - `close` can be a struct with `snap_to`, or `stale_marker`.
  - It needs a `duration`.
  - With `snap_to`, a closed gate gives `held` instead of `paused`.
- Cross-POST `while:` adds `scenario_name`, the entry `ref`, and `if_unresolved: open|closed|pending` (default `pending`).
  - It works only on `sonda-server`; `sonda run` rejects it.
  - Both bodies must declare a top-level `scenario_name`.
  - Resolution happens when a matching body is posted, not by polling.
  - Docs Patterns A, B and C describe baseline and cascade orchestration.
- Packs are `kind: composable` files with `name`, `description`, `category`, `shared_labels`, `metrics` and optional `extends` and `deviations`.
  - A runnable references one with `pack: <name>`, which expands to one entry per metric.
  - `overrides` is a map keyed by `name` or `name.id`.
  - Built-in packs are embedded in the binary. (`reference/sonda/docs/site/docs/build/builtin-packs.md` exists; I did not read it.)
- A catalog is a directory walked recursively for `kind:` headers.
  - `@name` resolves by `scenario_name` or filename.
  - Duplicate names are a hard error.
  - Files without a `kind` are skipped silently.
- **How files get into a running server:** there are three ways.
  - The client reads the file and POSTs the body, as `curl --data-binary` does. This is the only runtime path.
  - `--catalog` lets posted bodies use `pack:` names.
  - `--autostart` starts every `kind: runnable` file in the catalog at boot, in catalog-name order. A bad file is skipped; duplicate names stop the boot.
- Without `--catalog`, a body that names a pack is rejected with 400. The pack's metrics can be inlined instead.

### 4. Watching a scenario while it runs

- Only polling is documented, using these four reads:
  - `GET /scenarios` for state, elapsed time and `degraded`.
  - `GET /scenarios/{id}/stats` for rate, events, gap and burst flags, and sink health.
  - `GET /scenarios/{id}/metrics` or `/scenarios/metrics` for current values.
  - `GET /metrics` for server health, cap use, request counts and uptime.
- The observability routes (`/metrics`, `/scenarios/metrics`, `/scenarios/{id}/metrics`, `/scenarios/{id}/stats`, `/health`, `/ready`) are exempt from the request-limit guards, so they stay reachable when the control plane is saturated. (`reference/sonda/docs/site/docs/deploy/http-api.md`)
- Scenario output goes to its sink, not back to the API. With `sink: stdout` on a server, the output lands in the server's own log (`docker logs`). (`reference/sonda/docs/site/docs/deploy/docker.md`)
- Server logs are not available over HTTP. Skipped autostart files and loopback warnings appear only in the server log. The loopback warning also appears in the POST response.

### 5. How the workshops use Sonda

- **Address and auth:**
  - Every command defaults to `http://localhost:8085` and reads the URL from `SONDA_SERVER_URL`. (`reference/workshops/workshops/autocon5/src/autocon5_workshop/flap.py`, `incident.py`, `scenarios.py`, `reset.py`)
  - The setup script defaults to `http://sonda-server:8080`, the in-network address. (`reference/workshops/workshops/autocon5/sonda/scripts/sonda-setup.sh`)
  - `flap` and `incident` send a Bearer token from `SONDA_API_KEY`.
  - `scenarios`, `reset` and `status` send no auth header.
- **Endpoints used:**
  - `GET /health` (`reference/workshops/packages/nobs/src/nobs/commands/status.py`)
  - `GET /scenarios` (`scenarios.py`, `reset.py`)
  - `GET /scenarios/{id}` (the follow loops in `flap.py` and `incident.py`; `reset.py` for `pending_ref`)
  - `POST /scenarios` with a JSON body, timeout 10 s
  - `DELETE /scenarios/{id}` (`reset.py`)
  - `POST /events`, only by inference. `reference/workshops/packages/nobs/src/nobs/commands/maintenance.py` says its Loki annotation "routes through sonda /events" via `LokiClient.annotate`, but I did not read that client.
  - Not used: `/ready`, `/stats`, `/metrics`, per-scenario `/metrics`, `/gnmi/targets`, `/snmp/agents`.
- **`nobs autocon5 flap-interface`** (`flap.py`):
  - It builds JSON bodies in Python and POSTs one body per scope: one for the interface and one per healthy BGP peer.
  - Each body has `scenario_name` `autocon5-cascade-<device>-intf-<interface with / as ->` or `autocon5-cascade-<device>-bgp-<peer with . as ->`.
  - Entries: a `cascade_active` signal (a `flap` generator with `up_value:0`, `down_value:1`), constant "down" overrides gated by a local `while: cascade_active > 0`, and a Loki log entry (rate 0.5, `delay.open` = cascade delay, template message, json_lines encoder, loki sink).
  - The BGP flap is phase-shifted by `cascade_delay` (default 10 s): its `up_duration` becomes up + delay and its `down_duration` becomes down − delay.
  - Defaults: duration 4m, up 30s, down 60s.
  - Two peers are skipped as "broken": `("srl1","10.1.99.2")` and `("srl2","10.1.11.1")`.
  - `--follow` polls `GET /scenarios/{id}` every 5 s until the state is `finished` or `stopped`.
- **`nobs autocon5 incident`** (`incident.py`):
  - It POSTs one `link-failover` body named `incident-link-failover-<device>`.
  - The three entries are `primary_flap` (a `flap` generator, 60s up and 30s down), `backup_saturation` (`after` primary < 1) and `latency_degrade` (`after` backup > 70).
  - It sends over `remote_write` encoder and sink to Prometheus, with label `source=incident-cascade`.
- **`nobs autocon5 scenarios`** (`scenarios.py`): `GET /scenarios` and a Rich table of `id`, `name`, `state` and `elapsed_secs`. The code accepts a bare list, `{"scenarios":[...]}` or an id-keyed map; the docs describe only the `{"scenarios":[...]}` shape.
- **`nobs autocon5 reset`** (`reset.py`):
  - It lists scenarios and DELETEs those whose `name` is `cascade_active`, `incident_backup_link_utilization`, `incident_latency_ms` or `updown_logs_down`.
  - It then scans `paused` scenarios whose `pending_ref.scenario_name` starts with `autocon5-cascade-` or `incident-link-failover-`.
  - If it finds any, it runs `docker compose --project-name autocon5 restart sonda-server`, sleeps 5 s, and re-runs `up sonda-setup`.
  - It also re-runs `up sonda-setup` on its own to re-apply baselines, and restarts `sonda-logs` if Loki shows no `pipeline="direct"` srl1 logs in 60 s.
- **The setup container:**
  - `sonda-setup.sh` waits for `/health` (30 tries, 2 s apart) and reads each `*.yaml` in `$SCENARIOS_DIR` with PyYAML.
  - It POSTs each file as JSON to `/scenarios`.
  - It treats a 409 as "already registered".
  - It exits 1 if any other POST fails.
- **The workshop catalog** (`reference/workshops/workshops/autocon5/sonda/catalog/`) has 11 files:
  - `all-logs`, `cascade-incident`, `cisco-snmp-bgp-raw`, `cisco-snmp-interface-raw`, `srl1-metrics`, `srl2-metrics`, `srlinux-gnmi-bgp`, `srlinux-gnmi-bgp-raw`, `srlinux-gnmi-interface`, `srlinux-gnmi-interface-raw`, `srlinux-ping`.
  - Only `srl1-metrics`, `cascade-incident` and `all-logs` were read.
  - `srl1-metrics.yaml` uses `duration: 8760h`, `pack:` entries and cross-POST `while:` with `if_unresolved: open`, plus `delay.close.snap_to` overrides.
  - Per its comments, Telegraf scrapes `/scenarios/metrics` with `include_state=running,unresolved`, filtered by device label.
  - The workshop files use top-level `category:` on runnable files; the metadata table in `scenario-files.md` does not list it for runnables.
- **Status check:** `nobs status` probes `/health` on 7 services, including Sonda, with a 3 s timeout.

### 6. Documented as not built, removed or version-gated

- Raw-file log replay is "planned", tracked as sonda issue #347, in a separate adapter. (`reference/sonda/docs/site/docs/import/log-files.md`)
- `/events` "Not in this version": no burst path, no trace or flow types, no CLI subcommand. (`http-api.md`)
- Removed in 1.9: the subcommands `metrics`, `logs`, `histogram`, `summary`, `scenarios`, `packs`, `catalog`, `import` and `init`, and the flags and environment variables listed above. (`cli-flags.md`)
- Build-gated features: `remote_write`, `otlp`, `kafka`, `gnmi` and `snmp`. A request for a type that is not compiled in returns 422 with a hint. The published image includes remote-write, Kafka, gNMI and SNMP, but not OTLP. (`docker.md`, `http-api.md`)
- Behaviour that changed over time: `pending` was added later, `unresolved` and `held` came with cross-POST and `snap_to`, and the response shapes were extended. (`http-api.md`)
- Where nothing is marked: the grep for "experimental", "deprecated", "beta" and similar words found none on any API page.

## Reasoned conclusions

1. Everything the workshops do with Sonda is plain HTTP (list, inspect, POST, DELETE, health) plus one `/events` call. A terminal program needs no Sonda library for that. (Rests on §2 and §5.)
2. Watching a scenario means polling. Nothing is pushed to the client. The cheapest live view is `GET /scenarios` for all rows, plus `GET /scenarios/{id}/stats` for the selected row. (Rests on §2 and §4.)
3. The six lifecycle states must all be shown. The workshop table paints everything except `running` and `active` as a warning and tests for a `stopped` state that the docs do not define. (Rests on §2 and `scenarios.py`.)
4. "Load a file" has two meanings: send a local file to a server (any server, no restart), or make a catalog directory available to the server at boot. The first is client-only. The second needs access to the server's filesystem or container. A pack name in a posted body works only if the server was started with `--catalog`. (Rests on §3.)
5. A text user interface could do these from the documented API alone:
   - List the scenarios.
   - Show state, elapsed time, `degraded`, and the raw sink-failure fields.
   - Start from a local file.
   - Delete the selected row.
   - Send one event.
   - Show health, readiness and server load from `/metrics`.
   - Show the exact error text for 409, 422, 429 and 502, because those bodies explain themselves.
   - It cannot pause, resume, edit, tail output, or browse a remote catalog.
   (Rests on §2.)
6. Auth must be optional in the client: an open server needs no key; a keyed server needs one on every call except health and ready. (Rests on §1.)
7. The workshops' flap and incident logic lives in Python code that builds bodies in memory. It is workshop-specific and not part of Sonda. (Rests on §5.)

A text sketch of the data flow:

```
nobs (terminal) --HTTP--> sonda-server --push--> sinks (Prometheus, Loki, stdout, ...)
   |  POST /scenarios (file body)    |  holds scenario rows: pending|running|paused|held|unresolved|finished
   |  GET  /scenarios, /stats        |  --catalog DIR: pack names for posted bodies
   |  DELETE /scenarios/{id}         |  --autostart: start catalog files at boot
   |  POST /events (one signal)      |
```

Conclusions that still need checking: 3, 4 and 5. The documentation does not settle them. Items 3 and 4 need the Rust source named under the leads below.

## Open questions

- **Disagreements between sources:**
  - `reference/sonda/docs/site/docs/reference/cli-flags.md` says `--catalog` is required for `sonda list`. `reference/sonda/docs/site/docs/build/catalogs-and-packs.md` says built-in packs show "with no arguments".
  - The same two files list different `list --json` keys. `cli-flags.md` lists `name, kind, description, tags, source`. `catalogs-and-packs.md` adds `category, origin, shadows_builtin, extends, pack_error`. These may be a subset and a superset, but the text does not say so.
  - `reference/sonda/docs/site/docs/deploy/docker.md` says the shim handles four subcommands (`run, list, show, new`). `cli-flags.md` says there are six.
- **Gaps inside the docs:**
  - The Endpoint summary table in `http-api.md` leaves out `/gnmi/targets` and `/snmp/agents`, though both are documented in the page.
  - `?include_state` is documented only in `scenario-files.md`, not in the `http-api.md` section for `/scenarios/metrics`.
  - The docs show `scenario_name` default casing two ways: `scenario-files.md` says hyphens are preserved, `server.md` says underscores become dashes.
- **Behaviour reported against the docs:** `reset.py` says that after a cascade DELETE, Sonda "sometimes leaves them in `paused`" instead of `unresolved`. The docs say such rows return to `unresolved`. The Sonda version in that note is 1.13.1 (from the `flap.py` docstring); the docs describe 1.29.0. I did not verify which is right.
- **Not documented:**
  - Whether a running server re-scans `--catalog` for each POST.
  - Whether the server can validate a body without launching it.
  - Whether `POST /scenarios` returns the stream-count preview in `warnings` (`reference/sonda/docs/site/docs/build/scheduling.md` says "the response includes" it).
  - The exact shape of `GET /scenarios/{id}`: the docs say "config" without showing its fields.
  - Whether `category:` is accepted on a runnable file (the workshop files use it).
- **Leads not followed (all outside what I was allowed to read, or one level too far):**
  - `reference/sonda/sonda-server/src` (routes, auth, `GET /scenarios/{id}` body, catalog handling).
  - `reference/sonda/sonda-core/src` (scenario parser and compiler).
  - `reference/sonda/sonda-server/src/routes/scenarios.rs` (the `warnings` array and the scheduling stream-count preview).
  - `reference/sonda/docs/site/docs/schema/sonda-scenario.schema.json` (the machine-readable file schema).
  - Docs pages not read: `reference/sonda/docs/site/docs/build/builtin-packs.md`, `generators.md`, `encoders.md`, `sinks.md`, `reference/sonda/docs/site/docs/deploy/kubernetes.md`, `reference/sonda/docs/site/docs/reference/troubleshooting.md`, `reference/sonda/docs/site/docs/reference/glossary.md`, and the `test/` and `import/` folders.
  - Workshop files: `reference/workshops/packages/nobs/src/nobs/clients/loki.py` (the `/events` body), `reference/workshops/workshops/autocon5/src/autocon5_workshop/flap_topology.py`, the remaining catalog files, and the workshop's Compose file (for port 8085 and the `--catalog` mount).
  - Images and `javascripts/` in the Sonda docs folder.
- **Instructions found in sources:** none acted on. Comments such as "If you're considering fixing this ... plan for an Advanced-guide rewrite" in `incident.py` are notes to maintainers, not orders to me.

## Decisions for the architect

DECISION NEEDED: Which Sonda version must the first nobs support? The docs describe 1.29.0; the workshop code cites 1.13.1 behaviour. Default: target 1.29.0 and read the version from `sonda_server_build_info` on `/metrics` when the key allows it.

DECISION NEEDED: Is the first Sonda interface limited to the documented HTTP calls (health, ready, list, inspect, stats, delete, post a local file, post one event, metrics), or does it also shell out to the `sonda` CLI (`list`, `show`, `--dry-run`)? Default: HTTP only, so nobs works against a remote or containerized server without installing Sonda.

DECISION NEEDED: Does the first version include a local file check before posting? The server has no documented validate-only call, so this means the CLI's `--dry-run` or a Rust parser. Default: no local check; show the server's 400 or 422 text.

DECISION NEEDED: Where does the Sonda address and key come from? Default: flag first, then `SONDA_SERVER_URL` and `SONDA_API_KEY` (the names the workshops already use), with default URL `http://localhost:8080` (Sonda's own default) instead of the workshops' 8085; the key is never written to a config file.

DECISION NEEDED: Does nobs start, stop or restart `sonda-server` or the setup container, as `reset.py` does with `docker compose`? Default: no; it talks only to a running server, and lab lifecycle belongs to the lab-systems decision.

DECISION NEEDED: Are the workshop's own scenario builders (`flap-interface`, `incident`, `reset`) part of the first version, or only the generic "load a file and manage rows"? Default: generic only; the workshop flows ship as scenario files, not as hard-coded Rust.

DECISION NEEDED: Is polling acceptable for live views (for example, `/scenarios` and the selected `/stats` every 1 to 2 seconds)? Default: yes, since no push channel is documented.

DECISION NEEDED: Is Sonda's gNMI and SNMP surface (`/gnmi/targets`, `/snmp/agents`, and the `gnmi_target` and `snmp_agent` sinks) in or out of the first version? Default: out.

NOTE for the plan: nobody has run Sonda for this discovery. Every behaviour above is from documentation or workshop code, not from a live server.

CONFLICT: reference/sonda/docs/site/docs/reference/cli-flags.md and reference/sonda/docs/site/docs/build/catalogs-and-packs.md
CONFLICT: reference/sonda/docs/site/docs/deploy/docker.md and reference/sonda/docs/site/docs/reference/cli-flags.md
