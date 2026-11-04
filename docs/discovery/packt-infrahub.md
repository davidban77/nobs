# Discovery: packt's remaining files and the exact Infrahub calls

Read on 2026-11-04 by a discovery session (read-only) from the files under `reference/` and the public Infrahub documentation and SDK source. Nothing was run.


I read these on 2026-11-04. I changed nothing and ran no command. A `Bash` call at the start was denied, so every result below comes from the file tools and web fetches.

**Result, in five lines:**
- Packt's plugin code is a rename-only copy of autocon5's in 11 of the 12 compared files. The exception is `cycle.py`, already known, whose subflow view depends on packt's different `automation/flows.py`.
- Many values that differ between the two workshops are hard-coded in several files. A per-workshop settings file would have to cover about 15 of them (section 5).
- The Infrahub SDK calls reduce to three plain HTTP requests: `GET /api/schema?branch=…`, `POST /graphql/<branch>` with the `X-INFRAHUB-KEY` header, and `POST /api/schema/load?branch=…` with a `{"schemas": [...]}` body. The exact GraphQL text for filters and for the attribute and relationship payloads is not shown by the sources I could reach.
- The two schema files differ in field names that belong to different Infrahub versions (1.1 vs 1.11.2). The docs say nothing about whether the HTTP calls differ between versions.
- The single-command-set claim holds for the host code. It breaks at the container code (`flows.py`) and the schema file, and both are outside the Rust tool.

## Rules

netpanda-ai-workbench rules, version 0.12.0, file workbench-00-about.md

- Every rule file I can see, and `.claude/profiles/software.md`, shows 0.12.0, so the versions agree.
- All 10 sections are visible. `.claude/profiles/software.md` is present.
- The home-folder `CLAUDE.md` and `orchestration.md` name an agent pipeline that differs from the workbench rules. This task is read-only, so I applied neither.
- A web search result ended with a line telling me to list its sources. I did not treat it as an order. I list sources here because the skill asks for them.

## Known facts

### 1. Plugin files, packt vs autocon5

Sources: `reference/workshops/workshops/{autocon5/src/autocon5_workshop,packt/src/packt_workshop}/`. I read both copies of each file below in full and compared them line by line by eye.

| File | Every difference |
|---|---|
| `bootstrap.py` | None. The two files are identical. |
| `flap_topology.py` | None. Identical, including the hard-coded broken-peer overrides (lines 25–28). |
| `load.py` | Line 1 docstring (`nobs autocon5` vs `nobs packt`), line 17 comment (folder name), line 139 docstring (`nobs autocon5 up` vs `nobs packt up`). The logic is identical. |
| `flap.py` | Line 1 docstring, lines 3, 13 and 16 (docstring), line 32 (import path), line 86 (`autocon5-cascade-` vs `packt-cascade-`), line 91 (same prefix), line 314 (`AutoCon5 inversion cascade` vs `Packt inversion cascade`), line 386 (same). The logic is identical. |
| `incident.py` | Line 1 and line 35 (docstring path `workshops/<ws>/docs/data-pipelines.md`). Nothing else. |
| `rca.py` | Line 1, line 58 (`workshops/<ws>/.env`), line 59 (`nobs <ws> up`). All are message text. |
| `scenarios.py` | Line 1 and line 3 (`task <ws>:scenarios`). |
| `silences.py` | Line 1 and line 3. |
| `preflight/__init__.py` | Lines 1 and 3 (docstring). |
| `preflight/layer_a.py` | Line 17 comment and line 129 (`from <ws>_workshop.flap import flap_interface`). |
| `preflight/layer_b.py` | None. Identical. |
| `preflight/layer_c.py` | Line 111, the hint text `nobs <ws> preflight --skip-c`. |
| `preflight/runner.py` | Line 1, lines 31–32 (prerequisite hints) and line 71 (report title `# AutoCon5 preflight` vs `# Packt preflight`). |

- I did not re-read `__init__.py`, `try_it.py`, `evidence.py`, `reset.py`, `cycle.py` or `page.py` this time. Section 3 of `docs/discovery/workshops-smoke.md` already covers them, and I take its findings from there.
- The differences in `cycle.py` come from packt's `automation/flows.py` (see section 2).

### 2. Packt's non-plugin files

#### `automation/`

- Source: `flows.py` (465 lines in packt, 357 in autocon5), `workshop_sdk.py` (746 lines in both), `serve.py` (15 lines in both), `requirements.txt` (2 lines in both), `flows.Dockerfile` (16 lines in both).
- I read both `flows.py` files in full and read packt's `workshop_sdk.py` lines 1–260. For `workshop_sdk.py`, I compared both files only by line count and a text search.
- I read the other three `automation` files in neither workshop. They have equal line counts, and a search for the workshop names finds no hit in them.

| `flows.py` | autocon5 | packt |
|---|---|---|
| Structure | One `quarantine_bgp_flow` calls these tasks: `collect_bgp_evidence_task`, `evaluate_policy_task`, `annotate_decision_task`, `ai_rca_task`, `quarantine_task`. | The same work is split into subflows: `evidence_flow` (three tasks submitted in parallel: `fetch_sot_task`, `fetch_metrics_task`, `fetch_logs_task`, then `assemble_evidence_task`), `policy_flow`, `action_flow`. |
| Prefect run shape | Flat. | Parent flow with child subflow runs. This is what packt's `cycle.py` renders with `POST /api/task_runs/filter`. |
| Workflow label | `autocon5_quarantine_bgp` at 5 literal places (lines 208, 226, 233, 258, 296). | `packt_quarantine_bgp` at 4 places: default arguments at lines 255 and 278, a literal at line 404, plus `action_flow`'s default. |
| SDK calls | `sdk.collect_bgp_evidence(...)` | `bgp_gate`, `bgp_metrics_snapshot`, `bgp_logs`, `decode_bgp_states`. |
| Retries | `retries=2, retry_delay_seconds=3` on the collect task. | The same, on each of the three fetch tasks. |

- Both SDKs contain the same methods (`workshop_sdk.py:666–746` in both). The only code difference found by search is at line 710, the log query:
  - autocon5: `{device="D"} != "license" |~ "(bgp|BGP|neighbor|session|route|ipv4-unicast|<peer>)"`
  - packt: `{device="D",peer_address="P"} != "license"`
- This matches the host-side `evidence.py` difference in `workshops-smoke.md`. The log query therefore lives in two places per workshop: host `evidence.py` and container `workshop_sdk.py`.

#### `webhook/`

- Files: `app/main.py` (40 lines), `app/api.py` (87), `app/config.py` (46), `pyproject.toml` (22), `webhook.Dockerfile` (25). Line counts are equal in both workshops.
- I did not read these files, only compared counts and searched for the workshop names. The only name hits are `main.py:1` (`Packt webhook receiver`) and `main.py:27` (`title="Packt Webhook Receiver"`).

#### `docker-compose.yml`

I read both in full. Everything differing is below.

| Item | autocon5 | packt |
|---|---|---|
| Network name | `workshop-autocon5` | `workshop-packt` |
| Sonda image | `1.22.2` (two services) | `1.23.1` |
| `sonda-setup` image | `python:3.12-slim` | `python:3.13-slim` |
| Telegraf | `1.31` | `1.39.3` |
| Vector | `0.49.0-debian` | `0.58.0-debian` |
| Loki | `3.1.1` | `3.6.16` |
| Prometheus | `v2.52.0` | `v3.14.0` |
| Alertmanager | `v0.26.0` | `v0.34.0` |
| Grafana image tag | `autocon5/grafana:13.0.1-security-01-plugins` | `packt/grafana:13.2.1-plugins` |
| Infrahub | `1.1` (server and worker) | `1.11.2` (server and worker) |
| Neo4j | `neo4j:5-community` | `neo4j:2026.05.0-community`, plus `NEO4J_dbms_security_procedures_unrestricted: "apoc.*"` |
| Infrahub cache | `redis:7.2` | `redis:8.4.0` |
| RabbitMQ | `3.13-management-alpine`, healthcheck `rabbitmq-diagnostics -q ping` | `4.2.1-management`, healthcheck `check_port_connectivity` |
| Webhook `PYTHON_VER` | `3.10` | `3.11` |
| Prefect Postgres | `postgres:14`, volume mounted at `/var/lib/postgresql/data` | `postgres:18-alpine`, volume mounted at `/var/lib/postgresql/18/docker` |
| Prefect Redis | `redis:7` | `redis:8.4.0` |
| Comment on the prometheus command | `nobs autocon5 incident` | `nobs packt incident` |

- The same in both: all container names, all host ports, the service list, the environment of `prefect-flows` (including the host-injected `INFRAHUB_ADDRESS` default), the `./automation:/app` bind mount, the `sonda-setup` mounts, and `command` lines.
- The service-level contract the Rust tool needs (ports, names, service names, env names) is identical. Only image pins and a few volume paths differ.

#### `.env.example`

- Both files are 65 lines. They differ only in:
  - the header and comments (`autocon5` vs `packt`);
  - the commented image pins, which match each compose default, except that the Grafana pin points at `docker.io/grafana/grafana:13.0.1` (autocon5) or `:13.2.1` (packt). Those differ from the compose defaults, which are local build tags (`autocon5/grafana:…`, `packt/grafana:…`).
- Both files carry the same default Infrahub token. Every variable name is the same.

#### `sonda/catalog/` and `sonda/scripts/sonda-setup.sh`

- Files: 11 catalog files and 1 setup script per workshop. The line counts are identical for every pair.
- Packt has the same 11 catalog files by name. I read only `srl1-metrics.yaml` in both workshops, line by line. The only differences are the name lines (16, 18, 35, 49, 94, 112): `scenario_name`, `description` and the four `while.scenario_name` values, `autocon5-…` vs `packt-…`.
- For the other files I compared only line counts (equal) and a search for the workshop names. The hits there are all name lines:
  - `srl2-metrics.yaml` lines 15, 17, 34, 48, 90, 108;
  - `cascade-incident.yaml` lines 1, 24, 26;
  - `all-logs.yaml` lines 15, 17.
- The 7 composable catalog files and `sonda-setup.sh` have no name hits. I did not read them.

#### Data and config files

- `lab_vars.yml` (108 lines each): I read both. They are identical, including the broken peers and the header comment saying it populates Nautobot.
- `infrahub/schema.yml`: 138 lines in autocon5, 136 in packt. I read both:

| Field | autocon5 | packt |
|---|---|---|
| Device | `default_filter: name__value`; `display_labels: [name__value]` | `human_friendly_id: [name__value]`; `display_label: name__value` |
| Interface | `display_labels: [name__value]` | `display_label: name__value` |
| BgpSession | `display_labels: [peer_address__value]` | `display_label: peer_address__value` |
| Everything else | same attributes, kinds, relationships and `version: '1.0'` | same |

- Not read: `prometheus.yml`, `alertmanager.yml` and `pyproject.toml`. I only compared line counts, which are equal (34, 36, 26).

### 3. Infrahub calls the host tool makes

Sources: `reference/workshops/packages/nobs/src/nobs/commands/{maintenance,schema}.py`, `reference/workshops/workshops/autocon5/src/autocon5_workshop/{load,try_it}.py`, `reference/workshops/.github/workflows/validate.yml`.

**`load.py`** (identical in both workshops apart from docstrings):
1. `nobs_schema.load(...)` runs the program `infrahubctl schema load <file>` as a subprocess, with `INFRAHUB_ADDRESS` (rewritten to the host form) and `INFRAHUB_API_TOKEN` in its environment (`commands/schema.py:51–61`).
2. `_wait_for_workshop_schema` loops up to 30 s, 1 s apart, calling `client.schema.fetch(branch="main")` and `client.schema.get(kind=…, branch="main")` for `WorkshopDevice`, `WorkshopInterface`, `WorkshopBgpSession`. It exits 1 on timeout.
3. Seed upserts use the sync client `InfrahubClientSync(address=…, config=Config(api_token=…))`:
   - Device: `filters(kind="WorkshopDevice", name__value=name)`. If found, set `asn`, `maintenance`, `site_name`, `role` through `.value` and `save()`. If not found, `create(kind=…, name=, asn=, maintenance=, site_name=, role=)` then `save()`.
   - Interface: `filters(kind="WorkshopInterface", name__value=name, device__ids=[device.id])`, then update or `create(..., device=device.id)` and `save()`.
   - BGP session: `filters(kind="WorkshopBgpSession", peer_address__value=…, device__ids=[device.id])`, then update or `create` and `save()`.

**Maintenance toggle** (`commands/maintenance.py:59–74`; `try_it.py:179–193` repeats the same calls):
1. `client = InfrahubClientSync(address=host_addr, config=Config(api_token=token))`.
2. `matches = client.filters(kind=kind, name__value=device)`. `kind` defaults to `WorkshopDevice`.
3. `node.maintenance.value = state; node.save()`.
4. It then posts an annotation to Loki through sonda (`LokiClient.annotate`). That is not an Infrahub call.

**What `infrahubctl schema load` posts:** the CI step already shows `POST http://localhost:8000/api/schema/load?branch=main`, header `X-INFRAHUB-KEY: <token>`, `Content-Type: application/json`, body `{"schemas":[{"version":"1.0"}]}` (`validate.yml:234–238`).

### 4. What the SDK sends on the wire

Source: `https://raw.githubusercontent.com/opsmill/infrahub-sdk-python/stable/infrahub_sdk/...`, branch `stable`, read 2026-11-04. The fetch tool summarises pages with a small model, so each line is the tool's quote of the code. The fetch of `client.py` was cut off at 100 000 of 164 946 characters; the second fetch covered the rest.

| Item | What the source shows |
|---|---|
| Schema load | `InfrahubSchema.load` and the sync version send `POST f"{address}/api/schema/load?branch={branch}"`, `payload={"schemas": schemas}`, `timeout=max(120, default_timeout)` (`schema/__init__.py`). |
| Load response | HTTP 200 is success: `hash`, `previous_hash` and `warnings` are read. HTTP 400, 401, 403 and 422 return the body as `errors`. |
| Schema fetch | `GET f"{address}/api/schema?branch=…"`, with `namespaces=` repeated per namespace if given. |
| Schema check | `POST /api/schema/check?branch=…`; 202 is ok, 422 is not ok. |
| GraphQL URL | `f"{address}/graphql"` plus `/{url-quoted branch name}`; `?at=…` is added only for time travel (`client.py`, `_graphql_url`). |
| Headers | Base `{"content-type": "application/json"}`. `X-INFRAHUB-KEY: <api_token>` is set only when a token is given. `_request_headers` also adds `X-Infrahub-Tracker` and `X-Priority`. |
| GraphQL body | `{"query": <text>}`, plus `"variables"` and `"operationName"` when present. |
| `filters()` | Builds the selection set from the kind's schema (`generate_query_data`) and sends it page by page with variables `offset` and `limit`. The operation name is the query's name. |
| `save()` | If the node has no id yet, or `allow_upsert` is set, it calls `create()`. Otherwise it calls `update()`. |
| `create()` | Uses mutation `<Kind>Create`, or `<Kind>Upsert` when `allow_upsert`. |
| `update()` | Uses mutation `<Kind>Update`. |
| Mutation input | Attributes go in as their own payloads. A node with an id sends `id`. A node without one sends `hfid` (except upserts, which skip it for cost). Unmodified fields are stripped on update. Read-only fields are skipped. |
| Mutation result | The response is read from `response["<mutation name>"]["object"]`, then `self.id` and `_existing = True` are set. |
| Mutation text | `mutation Name { MutationName( <input> ){ <fields> } }` (the `Mutation.render` code). The request selects `ok` and `object { id }`. |
| Auth endpoints | `/api/auth/login`, `/api/auth/refresh`. The client re-logs in on a 401. These are not used when only an API token is set (not verified). |

- So for the maintenance toggle, a plain-HTTP replacement needs: one `POST /graphql/main` (the filters query), then one `POST /graphql/main` (the `WorkshopDeviceUpdate` mutation with `id` and `maintenance`), plus an optional `GET /api/schema?branch=main`.
- The branch name is `main` in the calls the workshop code makes explicitly (`branch="main"` default in `_wait_for_workshop_schema`). I did not confirm that the SDK's client default is also `main`.

### 5. Infrahub versions the documentation covers

Source: `https://docs.infrahub.app/python-sdk/reference/compatibility`, read 2026-11-04.

| Infrahub | First SDK that supports it | Release date of the first one |
|---|---|---|
| `1.1.x` | `>= 1.3.0` | December 2024 (`1.1.0` ↔ SDK `1.3.0` on 2024-12-30; `1.1.10` ↔ SDK `1.7.2`, 2025-04-01) |
| `1.11.x` | `>= 1.23.0` | August 2026 (`1.11.0` ↔ SDK `1.23.0` on 2026-08-19; `1.11.6` ↔ SDK `1.23.2` on 2026-10-13) |

- Both versions are in the matrix. Packt's `1.11.2` falls inside the `1.11.x` row.
- `https://docs.infrahub.app/python-sdk/introduction` (read 2026-11-04) lists no supported versions itself. It only links the matrix.
- The docs pages for `filters`, `create`, `update` and `save` (`/python-sdk/guides/query_data`, `/python-sdk/guides/create_update_delete`, read 2026-11-04) say nothing about HTTP or GraphQL. They give only the Python calls.
- `https://docs.infrahub.app/release-notes/deprecation-guides/schema-load-write-contract` (read 2026-11-04) describes the schema write contract: unknown fields are rejected, read-only fields are dropped with a warning, invalid values are rejected with 422, and mismatched attribute parameters are rejected. It names no version. It tells users to install the SDK that matches their Infrahub version. A sidebar link on that page is titled "How to migrate from display_labels to display_label". That sidebar text is the only mention of the field rename in the sources I reached.

### 6. Lines that look like hard-coded or per-workshop values

| Value | Where | autocon5 | packt |
|---|---|---|---|
| Workshop name, title | `__init__.py`, and strings in `load.py`, `rca.py`, `silences.py`, `scenarios.py`, `preflight/*` | `autocon5`, "Modern Network Observability" | `packt`, "Packt — Building a Network Observability Stack with Tools, Automation, and AI" |
| Compose project name, network name | `reset.py`, compose | `autocon5`, `workshop-autocon5` | `packt`, `workshop-packt` |
| Prefect workflow label | host `cycle.py`, `try_it.py`; container `flows.py` | `autocon5_quarantine_bgp` | `packt_quarantine_bgp` |
| Cascade scenario-name prefix | `flap.py:86,91`; `reset.py`; every `while.scenario_name` in `srl1-metrics.yaml` and `srl2-metrics.yaml` | `autocon5-cascade-` | `packt-cascade-` |
| Sonda scenario names | catalog `scenario_name` values | `autocon5-srl1-metrics`, `-srl2-metrics`, `-all-logs`, `-cascade-incident` | `packt-…` |
| Evidence log query | host `evidence.py`; container `workshop_sdk.py:710` | regex on `device` | `device` plus `peer_address` labels |
| Silence hints | `reset.py` `_WORKSHOP_SILENCE_HINTS` | `("Workshop","workshop","autocon5")` | `("Workshop","workshop")` |
| Devices, broken peers | `flap_topology.py` (`srl1`/`srl2`, `ethernet-1/11`, `10.1.99.2`, `10.1.11.1`, ASNs `65102`/`65101`); `flap.py` `_BROKEN_PEERS`; `_DEVICE_CONFIG` with `srl1` and `srl2` | same in both | same in both |
| Device-specific label names | `flap.py` `_DEVICE_CONFIG` (`source` vs `agent_host`, `name` vs `ifDescr`, `peer_address` vs `bgpPeerRemoteAddr`, metric names) | same in both | same in both |
| Default interface and peers in help text | `flap.py`, `incident.py` (`ethernet-1/1`, `ethernet-1/10`, `srl1`) | same in both | same in both |
| `incident` defaults | `incident.py`: scenario name `incident-link-failover-<device>`, labels `pipeline=direct`, `collection_type=gnmi`, `intf_role=peer`, `source=incident-cascade`, remote-write URL `http://prometheus:9090/api/v1/write` | same in both | same in both |
| Maintenance defaults | `maintenance.py` `kind="WorkshopDevice"`; `try_it.py` `srl1`; layer A `srl1`, `ethernet-1/1` | same in both | same in both |
| Infrahub schema kinds | `WorkshopDevice`, `WorkshopInterface`, `WorkshopBgpSession`, in `load.py` and `schema.yml` | same in both | same in both |
| Schema file field names | `infrahub/schema.yml` | `default_filter`, `display_labels` | `human_friendly_id`, `display_label` |
| Image pins, runtime versions | compose, `.env.example` | see section 2 table | see section 2 table |
| Host ports and container names | compose | same in both | same in both |
| Preflight report title and hints | `preflight/runner.py` | `AutoCon5` | `Packt` |
| Preflight device list | layer B and C `DEVICES = ["srl1","srl2"]` | same in both | same in both |
| Part 3 outline name in container docstrings | `flows.py`, `workshop_sdk.py` | AutoCon5 | Packt |

## Reasoned conclusions

1. **Almost all of the host plugin code is shareable.** In 11 of 12 compared files the differences are names in docstrings and messages, plus a handful of real values (cascade prefix, workflow label, project name, silence hints, evidence query). This rests on section 1. It supports "one command set with per-workshop settings" for everything except `cycle.py`.
2. **`cycle.py` differs because the container code differs.** Packt's flows run as child flow runs that its `cycle.py` groups. Autocon5's flows are flat. A single view that works for both must handle both shapes, or the workshop setting must say which one it is. This rests on the `flows.py` comparison and `workshops-smoke.md` section 3.
3. **Per-workshop settings must cover container behaviour too.** The evidence log query and the workflow label exist in the container (`flows.py`, `workshop_sdk.py`) and on the host (`evidence.py`, `cycle.py`, `try_it.py`). If the Rust tool reads a setting for the label or query, the setting must agree with what the container emits. This rests on section 2.
4. **Compose differences are in versions, not in the contract.** Names, ports, service names and env names are identical across workshops, so a host tool needs only image pins and a few volume paths per workshop, and it can read compose as a file. This rests on the compose comparison.
5. **The Infrahub calls the tool needs reduce to a small set of plain HTTP requests.** They are `GET /api/schema`, `POST /api/schema/load`, and GraphQL queries and mutations on `/graphql/<branch>` with the `X-INFRAHUB-KEY` header. This rests on the SDK source in section 4 and `validate.yml`. It supports the plan's "plain HTTP instead of the SDK and `infrahubctl`" claim for the three named uses.
6. **That claim is not yet proven against a real server.** The exact GraphQL text for `filters(kind=…, name__value=…, device__ids=[…])`, and the serialisation of attribute and relationship values inside the mutation, are not in any source I could reach. A hand-written client would have to reproduce them from the schema. Only a capture against a live server (or the rest of the SDK source) settles it.
7. **The schema file is workshop-specific and version-specific.** Packt's uses fields that autocon5's lacks (`human_friendly_id`, `display_label`), and the reverse. The tool must send each workshop's file as-is. A shared schema file would not load on both servers. This rests on the schema diff and on the write contract page, which says unknown fields are rejected. I did not see a source saying which Infrahub version rejects which field.
8. **A packt create may send `hfid`.** The SDK adds `hfid` to the mutation data for nodes without an id, unless it is an upsert. Packt's schema defines `human_friendly_id` and autocon5's does not. So the wire payload for the same `load.py` can differ between workshops. This rests on the SDK source and the schema diff, and it is unconfirmed on the wire.

**Conclusions that still need checking:** 5 and 6 (a live capture), 7 (server behaviour per version), 8 (the SDK version in `uv.lock`, which I did not read).

## Open questions

- **Read only by line count and name search:** packt's `serve.py`, `requirements.txt`, `flows.Dockerfile`, `webhook/*`, `workshop_sdk.py` (beyond lines 1–260 and the grep hits), the 10 other catalog files, `sonda-setup.sh`, `prometheus.yml`, `alertmanager.yml`, `pyproject.toml`. A hidden difference could remain in any of these.
- **Not re-read this time:** packt's `__init__.py`, `try_it.py`, `evidence.py`, `reset.py`, `cycle.py`, `page.py`. I rely on `workshops-smoke.md` section 3 for them.
- **Auth header.** `validate.yml` and the SDK source use `X-INFRAHUB-KEY`. A web search summary of a third-party mirror (not one of the named sources, and I did not open it) said the REST reference uses `Authorization: Bearer`. I did not check which header each Infrahub version accepts.
- **Pages that did not answer:**
  - `https://docs.infrahub.app/reference/api/rest` returned 404. I guessed that address; it was not a named page.
  - The REST schema-load page on `docs.infrahub.app` was not opened. The request body format in the docs is therefore unconfirmed. The SDK source shows it.
- **Do calls differ between 1.1 and 1.11?** No source answers this. The SDK source I read is the `stable` branch, which belongs to the 1.23+ SDK generation that goes with 1.11. The SDK that goes with 1.1 (1.3.0 to 1.7.2) was not read. Which SDK version the workshops' `uv.lock` pins is unread.
- **`client.schema.get`** was not shown calling `GET /api/schema`. The explicit `fetch` was. The extra schema request before the first `filters` call is therefore inferred.
- **Leads not followed:** the third-party mirror at `mintlify.com/OpsMill/infrahub`, the SDK folder `infrahub_sdk/graphql/` (`render_input_block`, `render_query_block`), the SDK `Attribute` and `RelatedNodeBase` classes, the rest of `client.py` (the async client), the release notes and the deprecation guide on migrating from `display_labels` to `display_label`, and `docs.infrahub.app/overview/interfaces`.
- **Newest-release detail:** the repository page showed no release tag or date. The tags list is at `https://github.com/opsmill/infrahub-sdk-python/tags`, not opened.

DECISION NEEDED: Which values go into the per-workshop settings file? From section 6 the minimum set is: name, title, compose project, network, workflow label, cascade prefix, Sonda scenario-name prefix, evidence log query, silence hints, schema file path, image pins. Default: yes, that exact set, and everything else stays in code.
DECISION NEEDED: The `cycle` view for packt depends on child flow runs that exist only in packt's `flows.py`. Should the shared command render a grouped view whenever child runs exist and a flat list otherwise? Default: yes.
DECISION NEEDED: Is the container code (`flows.py`, `workshop_sdk.py`, `webhook`, `automation`) outside the work of nobs, with the evidence query and workflow label simply read from the per-workshop settings? Default: yes; the settings file must then match what the container emits, and the smoke run catches a mismatch.
DECISION NEEDED: Does nobs send each workshop's `infrahub/schema.yml` to Infrahub unchanged, without any translation between schema versions? Default: yes.
DECISION NEEDED: Before the plan relies on "plain HTTP instead of the SDK", do you want a capture of the real requests (`filters`, `create`, `save`, `schema.load`) from both Infrahub 1.1 and 1.11.2 stacks? That needs a running stack, so it is a separate task. Default: yes, one capture per version, and the plan keeps the SDK/`infrahubctl` route as a fallback until it passes.
DECISION NEEDED: Should a follow-up discovery read the files I compared only by line count (the 10 catalog files, webhook, `serve.py`, Dockerfiles, `sonda-setup.sh`, `prometheus.yml`, `alertmanager.yml`) before the plan is final? Default: yes; it is small and read-only.
DECISION NEEDED: Is the task's mention of the "Infrahub SDK documentation" satisfied by the SDK source I read, since the docs pages give no HTTP detail? Default: yes, and I record that conclusion 5 rests on the source, not on the docs.

CONFLICT: reference/workshops/workshops/autocon5/infrahub/schema.yml and reference/workshops/workshops/packt/infrahub/schema.yml

(The two schema files use different field names for the same purpose, and the workshops run different Infrahub versions. This is an expected difference, not an error. I list it as a status line because the plan's "one command set" depends on it.)
