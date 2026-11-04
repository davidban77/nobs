# Discovery: the smoke job, the containers, the two plugins, the Sonda catalog and response shape

Read on 2026-11-04 by a discovery session (read-only). Nothing was run.

# Discovery: smoke job, container needs, plugin differences, Sonda catalog and response shape

I read the sources on 2026-11-04. Nothing was run. I tried one `find`/`wc` shell command at the start, and it was denied, so no command ran. Everything below comes from reading files.

**Result, in four lines:**
- The smoke job is a fixed sequence of 10 steps. Its two "wait" gates do not fail the job, and `try-it --auto` exits 0 even when all four paths miss.
- The containers need only environment variables passed through compose, plus one bind-mounted folder. Nothing in them calls the host tool.
- The two plugins differ by more than the known list. `cycle.py` has real logic differences.
- Sonda's `GET /scenarios/{id}` returns no `config` object, though the docs (as relayed in `docs/discovery/sonda.md`) say it does.

## Rules

netpanda-ai-workbench rules, version 0.12.0, file workbench-00-about.md

- Every rule file I can see, and `.claude/profiles/software.md`, shows 0.12.0, so the versions agree.
- All 10 sections are visible.
- The home-folder `CLAUDE.md` and `orchestration.md` name agents and a pipeline that differ from the workbench rules. This task is read-only, so I applied neither.

## Known facts

### 1. The scheduled smoke job

Source: `reference/workshops/.github/workflows/validate.yml`. The file carries no version.

**When it runs:**
- Daily at 06:30 UTC (`cron: "30 6 * * *"`).
- On `workflow_dispatch`, when the input `run_smoke` is `"true"` (the default).
- Never on a pull request or a push.

**Job settings:**
- `needs: lint`, `runs-on: ubuntu-latest`, `timeout-minutes: 25`.
- A matrix over `workshop: [autocon5, packt]` with `fail-fast: false`. One runner per workshop, because both stacks use the same container names and ports.
- A concurrency group `<workflow>-<ref>` with `cancel-in-progress: true`.
- There is no `env:` block anywhere in the workflow. The only environment comes from the workshop's `.env` file.

**Steps, in order:**

| # | Step | Exact content |
|---|---|---|
| 1 | Checkout | `actions/checkout@v4` |
| 2 | uv | `astral-sh/setup-uv@v3`, `enable-cache: true`, cache key from `**/pyproject.toml` and `uv.lock` |
| 3 | uv sync | `uv sync --all-packages` |
| 4 | Bring stack up | `uv run nobs $WS setup`, then `uv run nobs $WS up` |
| 5 | Wait for stack readiness | See below |
| 6 | Seed Infrahub | `uv run nobs $WS load-infrahub` |
| 7 | Wait for first alert | See below |
| 8 | Walk the four Part 3 paths | `uv run nobs $WS try-it --auto` |
| 9 | Dump container logs | Only `if: failure()`. See below. |
| 10 | Tear down | `if: always()`: `uv run nobs $WS destroy` |

**Step 4, what the two commands do** (`nobs/main.py`, `lifecycle/commands.py`, `lifecycle/compose.py`, `lifecycle/setup.py`):
- `$WS setup` only copies `.env.example` to `.env` if `.env` is missing. It runs no `uv sync` and no preflight.
- `$WS up` does these things in order:
  1. Runs `docker ps -a --filter label=com.docker.compose.project=<other>` for the other workshop, and exits 1 if containers exist.
  2. Runs the bootstrap again.
  3. Runs `docker compose --project-name $WS -f <ws>/docker-compose.yml up -d`, with the workshop folder as cwd and the process environment passed on. There is no `--build`.
  4. Exits with compose's exit code if it is not 0.

**Step 5, the waits.** The step has no `set -e`, and each loop is followed by a final check.

| Gate | Loop | Pass test | On failure |
|---|---|---|---|
| Prometheus `:9090/-/ready` | up to 60 tries, 5 s sleep between | `curl -sfo /dev/null --max-time 3` | `exit 1` |
| Loki `:3001/ready` | same | same | `exit 1` |
| Alertmanager `:9093/-/ready` | same | same | `exit 1` |
| Infrahub phase 1 | up to 60 tries, 5 s sleep | `curl -sf --max-time 3` on `:8000/health` or `:8000/api/healthcheck` | `exit 1` |
| Infrahub phase 2 | up to 60 tries, 5 s sleep | after `set -a; . workshops/$WS/.env; set +a`: `curl -s --max-time 5 -X POST` to `:8000/api/schema/load?branch=main`, header `X-INFRAHUB-KEY: $INFRAHUB_API_TOKEN`, body `{"schemas":[{"version":"1.0"}]}`, expecting HTTP 200 | `exit 1`, printing the last code |

- Because `curl -f` is used, a Loki 503 during its warm-up counts as not ready. The Python `try-it` accepts that 503 (see section 2), so the two disagree.

**Step 6, what `load-infrahub` does** (`autocon5_workshop/load.py`, `commands/schema.py`):
1. Requires `INFRAHUB_API_TOKEN`; exits 1 if missing.
2. Rewrites `INFRAHUB_ADDRESS` to `http://localhost:8000` when it contains `infrahub-server`.
3. Runs `infrahubctl schema load infrahub/schema.yml` as a subprocess. Exits with its code on failure.
4. Waits up to 30 s, polling every 1 s, for the three workshop kinds to appear. Exits 1 if they do not.
5. Upserts devices, interfaces and BGP sessions through the Infrahub SDK.

**Step 7:**
- Up to 24 iterations of: `curl -sf http://localhost:9090/api/v1/alerts`, count firing `BgpSessionNotUp` alerts with an inline `python3` one-liner, `break` if the count is above 0, otherwise `sleep 5`. That is at most about 120 s.
- There is no `exit 1` after the loop. The step passes whether or not an alert fired.

**Step 8, what `try-it --auto` does** (`autocon5_workshop/try_it.py`):
1. Exits 1 if `INFRAHUB_API_TOKEN` is empty.
2. Rewrites the Infrahub address to the host form.
3. Preflight: `GET /-/ready` on Prometheus, `/ready` on Loki and `/-/ready` on Alertmanager, each with a 3 s timeout. A Loki 503 whose body contains "Ingester not ready" counts as ready. Any failure exits 1. Then Infrahub `/api/healthcheck`, then `/health`, 3 s each. If neither answers, exit 1.
4. `time.sleep(2)` after each stage in `--auto` mode.
5. The four paths run one after another:

| Path | Action | Waits for (in Loki, 60 s timeout) |
|---|---|---|
| 1 | POST firing alert srl1 → 10.1.99.2 | `{source="prefect", workflow="<ws>_quarantine_bgp", device="srl1", peer_address="10.1.99.2", decision="proceed"}` |
| 2 | Set srl1 `maintenance=true` (Infrahub SDK), POST same alert, then set `maintenance=false` | `{..., device="srl1", decision="skip"}` |
| 3 | POST firing alert srl1 → 10.1.2.2 | `{..., device="srl1", peer_address="10.1.2.2", decision="skip"}` |
| 4 | POST resolved alert srl1 → 10.1.99.2 | `{..., device="srl1", decision="resolved"}` |

- Each POST goes to `WEBHOOK_URL` (default `http://localhost:9997/v1/api/webhook`) with a 15 s timeout.
- Each wait polls Loki every 5 s, up to 60 s. Each poll queries the last 5 minutes with limit 1000 and counts lines.
- **Exit codes.** `ok`, `warn` and `fail` in `nobs/_console.py` only print. A webhook POST error only warns. `_wait_for_loki` ends with `fail(...)`, which does not exit. So `try-it --auto` exits 0 even if all four waits time out. It exits non-zero only for a missing token, an unreachable stack service, a missing `infrahub-sdk`, or an unhandled exception from the SDK calls.

**Step 9:** in `workshops/$WS`, for `prefect-flows webhook alertmanager prometheus loki infrahub-server`, run `docker compose logs --tail=100 <svc> || true`.

**Step 10:** `docker compose down --volumes --remove-orphans` for the project.

**Retries.** The workflow has no step-level retries. The only loops are those above. Inside the stack:
- `prefect-worker` and `prefect-flows` have `restart: on-failure` in compose.
- Prefect's evidence task has `retries=2, retry_delay_seconds=3` (`automation/flows.py`).
- The webhook calls `run_deployment(..., timeout=10)` (`webhook/app/api.py`).

**How the Python nobs is installed and invoked:**
- `uv sync --all-packages` installs the uv workspace (Python 3.11 or newer).
- `uv run nobs <ws> <command>` runs from the repo root.
- The workshop is picked by name from a registry that fills at import time.
- Each command loads `workshops/<ws>/.env` into the process environment first. The process environment wins over the file (`lifecycle/env.py`).

### 2. What the containers need from the host

**Prefect flows container (`prefect-flows`)**, from `docker-compose.yml` lines 456–484:
- **Environment, literal:** `PREFECT_API_URL=http://prefect-server:4200/api`, `PROMETHEUS_URL=http://prometheus:9090`, `LOKI_URL=http://loki:3001`, `ALERTMANAGER_URL=http://alertmanager:9093`, `SONDA_SERVER_URL=http://sonda-server:8080`.
- **Environment, taken from the host or `.env` by compose substitution:**
  - `INFRAHUB_ADDRESS` (default `http://infrahub-server:8000`).
  - `INFRAHUB_API_TOKEN` (no default).
  - `SONDA_API_KEY` (empty by default).
  - `ENABLE_AI_RCA` (false).
  - `AI_RCA_PROVIDER` (openai).
  - `AI_RCA_MODEL` (gpt-4o-mini).
  - `OPENAI_API_KEY` and `ANTHROPIC_API_KEY` (empty).
  - `SONDA_LOKI_SINK_URL` is not set, so the SDK default `http://loki:3001` applies.
- **File:** the bind mount `./automation:/app`.
- **Caveat:** if the host process exported a rewritten `INFRAHUB_ADDRESS`, compose would inject it into this container and break it. That is why `host_address()` does not touch the process environment.

**Webhook container:**
- Only `PREFECT_API_URL` (literal) and the build argument `PYTHON_VER` (default 3.10). It needs no host files.
- `webhook/app/main.py` raises at start-up if `PREFECT_API_URL` and `PREFECT_URL` are both unset or not an http(s) URL.
- It listens on 9997 and answers `POST /v1/api/webhook` with 204 even on internal errors. It catches all exceptions and logs them.

**AI step** (`automation/workshop_sdk.py`):
- It runs inside `prefect-flows`.
- With `ENABLE_AI_RCA` off, it writes a "disabled" annotation.
- With `demo`, it builds a templated text with no network call.
- With `openai` or `anthropic`, it calls `api.openai.com` or `api.anthropic.com` directly, with a 60 s timeout, using the keys from the environment.
- It writes the result to Loki through `POST sonda-server:8080/events`. The container does not call the host.

**Calls into the host tool:**
- No container runs `nobs` or `uv`. I found none in `automation/` or `webhook/`.
- The only `uv run nobs ...` calls are the host command `reset` calling itself (`reset.py`: `load-infrahub`, `maintenance --device <d> --clear`). They run with `cwd` set to the repo root, found by `Path(__file__).parents[4]`.

**Does `try-it --auto` need more than HTTP from the host?** Yes, in four small ways, and none is docker:
1. The `.env` merge supplies `INFRAHUB_API_TOKEN`.
2. The `INFRAHUB_ADDRESS` host rewrite.
3. The maintenance toggle uses the Infrahub SDK (`InfrahubClientSync.filters(...)` then `node.save()`). I did not read the SDK, so I do not know its exact HTTP calls.
4. `sys.exit(1)` if the SDK is not installed.
- It does not run docker or any subprocess.
- The request the webhook accepts must match this pydantic model, with all fields present:
  - Group: `version`, `groupKey`, `truncatedAlerts`, `status` (`firing` or `resolved`), `receiver`, `groupLabels`, `commonLabels`, `commonAnnotations`, `externalURL`, `alerts`.
  - Each alert: `status`, `labels`, `annotations`, `startsAt`, `endsAt`, `generatorURL`, `fingerprint`.

The chain that `try-it` exercises:

```
host nobs --POST--> webhook:9997 --run_deployment--> prefect-server
                                                        |
                                prefect-flows (alert_receiver -> quarantine_bgp_flow)
                                   |-- Infrahub GraphQL, Prometheus, Loki (read)
                                   |-- Alertmanager silence (write, "proceed" only)
                                   '-- sonda-server /events --> Loki (audit lines)
host nobs <--polls Loki-- {workflow="<ws>_quarantine_bgp", decision=...}
```

### 3. Plugin comparison

I read both copies in full for `__init__.py`, `evidence.py`, `reset.py`, `try_it.py`, `cycle.py` and `page.py`. For the other files I compared line counts and every line that contains `autocon5`, `packt` or their capitalised forms. I did not diff them line by line.

| File | Difference found |
|---|---|
| `__init__.py` | `name` and `title` (`Modern Network Observability` vs `Packt — Building a Network Observability Stack with Tools, Automation, and AI`), docstrings and comments. The 12 registered commands are identical. |
| `try_it.py` | The workflow label in the four Loki queries and the final panel. The "bring the stack up" hint (`nobs packt up`). The docstring. Nothing else. |
| `evidence.py` | The log query only. autocon5: `{device="D"} != "license" \|~ "(bgp\|BGP\|neighbor\|session\|route\|ipv4-unicast\|PEER)"`. packt: `{device="D",peer_address="P"} != "license"`. |
| `reset.py` | Four things, not one:<br>1. Project name, folder and `uv run nobs <ws>` strings.<br>2. `_WORKSHOP_SILENCE_HINTS`: autocon5 has `("Workshop","workshop","autocon")`, packt has `("Workshop","workshop")`.<br>3. The wedge-check prefix: `autocon5-cascade-` vs `packt-cascade-`.<br>4. Docstrings. |
| `cycle.py` | **Logic differs** (389 vs 440 lines):<br>- packt adds `_group_flow_runs` and `_add_run_row`. They call `POST /api/task_runs/filter` and render child subflow rows under a parent.<br>- packt shows `top[:3]`; autocon5 shows a flat `runs[:5]`.<br>- Plus the workflow label and help text. |
| `page.py` | Strings only (`nobs <ws> alerts`, runbook path). The final print is wrapped differently (134 vs 133 lines). |
| `flap.py` | The scenario-name prefix at lines 86 and 91 (`autocon5-cascade-` vs `packt-cascade-`), the import path, and the description text at lines 314 and 386. The grep hits sit on the same line numbers in both files, and the line counts match (464). |
| `incident.py`, `load.py`, `rca.py`, `silences.py`, `scenarios.py` | Names in docstrings and comments only. Same line numbers, same line counts. |
| `preflight/layer_a.py`, `layer_c.py`, `runner.py`, `__init__.py` | An import path, a hint, a report title and docstrings. Same line counts. |
| `bootstrap.py`, `flap_topology.py`, `preflight/layer_b.py` | No name hits in either copy. Same line counts. I read only the autocon5 `bootstrap.py`. |

- The new differences beyond the known list: the `cycle.py` logic, the `reset.py` silence hint, and the `reset.py` cascade prefix.
- The `flap.py` prefix (`packt-cascade-`) must match the `scenario_name` values in packt's own Sonda catalog. I did not read that catalog.

### 4. The 11 Sonda catalog files of autocon5

Source: `reference/workshops/workshops/autocon5/sonda/catalog/`. All 11 are `version: 2`.

| File | Kind | Name | What it holds | Cross-POST references |
|---|---|---|---|---|
| `all-logs.yaml` | runnable | `autocon5-all-logs` | 9 log scenarios. srl1: system logs, broken-interface logs, BGP logs for peers 10.1.2.2 and 10.1.7.2, broken-peer logs for 10.1.99.2, all pushed to the Loki sink. srl2: the same three streams plus the broken peer 10.1.11.1, as RFC 5424 syslog over UDP to `vector:1514`. | none |
| `cascade-incident.yaml` | runnable | `autocon5-cascade-incident` | A reference body. `primary_flap` (an `interface_oper_state` flap) gates six BGP metrics for peer 10.1.2.2 and one UPDOWN log, using a **local** `while`, `delay.open: 10s` and `snap_to`. Remote-write to Prometheus. | none (local `ref: primary_flap`) |
| `srl1-metrics.yaml` | runnable | `autocon5-srl1-metrics` | srl1 raw gNMI telemetry: BGP peers 10.1.2.2, 10.1.7.2 and the broken 10.1.99.2; interfaces ethernet-1/1, 1/10 and the broken 1/11; ping; CPU, memory and uptime. | `while.scenario_name`: `autocon5-cascade-srl1-bgp-10-1-2-2`, `...-10-1-7-2`, `autocon5-cascade-srl1-intf-ethernet-1-1`, `...-ethernet-1-10`, all with `ref: cascade_active`, `op: <`, `value: 1`, `if_unresolved: open` |
| `srl2-metrics.yaml` | runnable | `autocon5-srl2-metrics` | srl2 raw SNMP-style telemetry: BGP peers 10.1.2.1, 10.1.7.1 and the broken 10.1.11.1; interfaces ethernet-1/1, 1/10 and the broken 1/11; ping; CPU, memory and uptime. | Same pattern with `autocon5-cascade-srl2-bgp-10-1-2-1`, `...-10-1-7-1`, `autocon5-cascade-srl2-intf-ethernet-1-1`, `...-ethernet-1-10` |
| `srlinux-gnmi-bgp-raw.yaml` | composable | pack `srlinux_gnmi_bgp_raw` | 7 `srl_bgp_*` metrics, constants. | none |
| `srlinux-gnmi-bgp.yaml` | composable | pack `srlinux_gnmi_bgp` | The same 7 metrics with normalised `bgp_*` names. | none |
| `srlinux-gnmi-interface-raw.yaml` | composable | pack `srlinux_gnmi_interface_raw` | 4 `srl_interface_*` metrics (admin, oper, in/out octets as `step`). | none |
| `srlinux-gnmi-interface.yaml` | composable | pack `srlinux_gnmi_interface` | The same 4 with normalised `interface_*` names. | none |
| `cisco-snmp-bgp-raw.yaml` | composable | pack `cisco_snmp_bgp_raw` | 7 SNMP-style BGP metrics (`bgpPeerState`, `cbgpPeer*` and others). | none |
| `cisco-snmp-interface-raw.yaml` | composable | pack `cisco_snmp_interface_raw` | 4 IF-MIB metrics (`ifAdminStatus`, `ifOperStatus`, `ifHCInOctets`, `ifHCOutOctets`). | none |
| `srlinux-ping.yaml` | composable | pack `srlinux_ping` | 2 metrics: `ping_result_code`, `ping_average_response_ms`. | none |

- **Counts:** 4 runnable and 7 composable files.
- **Pack use:** `srl1-metrics` uses `srlinux_gnmi_bgp_raw`, `srlinux_gnmi_interface_raw` and `srlinux_ping`. `srl2-metrics` uses `cisco_snmp_bgp_raw`, `cisco_snmp_interface_raw` and `srlinux_ping`. No runnable file uses the two normalised packs `srlinux_gnmi_bgp` and `srlinux_gnmi_interface`.
- **Who creates `cascade_active`:** its scenario bodies are built in memory by `flap.py`. The catalog only references it.
- **Compose wiring** (`docker-compose.yml` lines 42–62):
  - `sonda-server` runs `--port 8080 --bind 0.0.0.0 --catalog /catalog`, so pack names resolve.
  - `sonda-setup` mounts and posts only `srl1-metrics.yaml` and `srl2-metrics.yaml`.
  - I did not read how `sonda-logs` uses `all-logs.yaml`.

### 5. Exact JSON shape of `GET /scenarios/{id}`

Source: `reference/sonda/sonda-server/src/routes/scenarios.rs`, lines 96–109, 137–156 and 866–902, plus the tests at lines 2125–2203 and 6481–6527. The crate version is Sonda 1.29.0, from the workspace `Cargo.toml` as relayed in `docs/discovery/sonda.md`.

Status 200:

```json
{
  "id": "string",
  "name": "string",
  "state": "pending | running | paused | held | unresolved | finished",
  "elapsed_secs": 12.345,
  "degraded": false,
  "stats": {
    "total_events": 0,
    "current_rate": 0.0,
    "bytes_emitted": 0,
    "errors": 0,
    "consecutive_failures": 0,
    "total_sink_failures": 0,
    "last_sink_error": null,
    "last_successful_write_at": null,
    "degraded": false
  },
  "pending_ref": { "scenario_name": "...", "entry_id": "..." }
}
```

- `pending_ref` is **omitted** (not `null`) unless `state` is `unresolved` (`skip_serializing_if = "Option::is_none"`).
- The type `sonda_core::PendingRef` lives in `sonda-core`, which I did not read. The test at line 6503 asserts only `scenario_name` and `entry_id`. It may have more fields.
- `last_sink_error` is a string or null. `last_successful_write_at` is Unix nanoseconds (u64) or null.
- `stats.degraded` and the top-level `degraded` carry the same value. The handler overwrites `stats.degraded` with the computed one.
- `state` is `"unknown"` if a future state variant exists (`state_string`, line 266).
- This `stats` object has 9 fields. It is **not** the `GET /scenarios/{id}/stats` body, which also has `target_rate`, `uptime_secs`, `state`, `in_gap`, `in_burst` and more.
- A missing id gives 404 with a message `scenario not found: <id>`. A test named `get_scenario_nonexistent_returns_404_with_json_body` says the body is JSON. I did not read the `not_found` helper, so the exact JSON keys are unknown.
- There is **no `config` field**.

## Reasoned conclusions

1. **The smoke job cannot detect a broken chain today.** It rests on `_console.py` (`fail` only prints), the end of `_wait_for_loki`, and the absence of an `exit 1` after step 7's loop. The job fails only when the stack does not start, the readiness gates time out, `load-infrahub` fails, or `try-it` hits one of its four early exits. A Rust port that copies the exit behaviour would inherit a test that passes while the Prefect chain is dead. This needs your decision (see the first `DECISION NEEDED` below).
2. **A Rust nobs can run the same sequence with host-side work only.** It must run `docker compose` with a project name, a compose file and a cwd, and make plain HTTP calls. The two non-HTTP dependencies are `infrahubctl schema load` (step 6) and the Infrahub SDK (steps 6 and 8). `validate.yml` already shows a direct HTTP route for the schema (`POST /api/schema/load`). This rests on section 1, section 2 and workshops.md conclusion 4.
3. **The compose stack, webhook and AI step stay unchanged.** They need only environment variables and the `./automation` mount, and none calls the host tool. This resolves the open point in workshops.md conclusion 2, which said `flows.py` was unread. This rests on section 2.
4. **The `.env` handling is part of the contract.** The host must load `.env`, give the process environment priority, and rewrite `INFRAHUB_ADDRESS` only per call. A Rust port that exports the rewritten value would break `prefect-flows` through compose substitution. This rests on `env.py` and compose lines 464–465.
5. **The two plugins are close but not identical.** One shared command set with per-workshop settings fits these facts: workflow label, project name, cascade prefix, silence hints, evidence query, title. It would also have to settle `cycle.py`, where only packt groups subflows. This rests on section 3. The files not read line by line could hide further differences.
6. **The Sonda interface should not show a "config" view of a scenario.** The server returns none. A detail view can show `id`, `name`, `state`, `elapsed_secs`, `degraded`, the 9 stats fields and `pending_ref`. This rests on section 5.
7. **Only runnable catalog files make sense to post as-is.** The 7 composable files are packs that a runnable body names with `pack:`. This needs the server to run with `--catalog`, as the workshop compose does. This rests on section 4 and the catalog line of `docs/discovery/sonda.md`.

Conclusions that still need checking: 1 (it needs one run to confirm), 5 (the unread files), and 7.

## Open questions

- **Conflict between a relayed doc and the code.** `docs/discovery/sonda.md` says the docs describe `GET /scenarios/{id}` as returning "Config, stats, elapsed time". `routes/scenarios.rs` returns no config. I did not read `http-api.md` myself. The code favours no `config`. See the `CONFLICT` line.
- **Not read:**
  - The `sonda_core::PendingRef` type.
  - The `not_found` helper (the 404 body).
  - The Infrahub SDK's HTTP calls.
  - Packt's `automation/`, `webhook/`, compose file, `.env.example` and catalog. The task named only packt `src`.
  - `sonda-logs`' command and `sonda-setup.sh`.
  - The Telegraf and Vector configs.
  - The guides.
- **Plugin files compared only by line count and name hits:** `bootstrap`, `flap`, `flap_topology`, `incident`, `load`, `rca`, `scenarios`, `silences`, and the four preflight files `layer_a`, `layer_b`, `layer_c`, `runner`.
- **Files outside the named sources that I read as one-level references from workshops.md:**
  - `nobs/_console.py`, `lifecycle/setup.py` and `lifecycle/compose.py`. These came from imports, so by the skill they are leads, not references.
  - `docker-compose.yml` and `.env.example`, which workshops.md names by path.
  - `lifecycle/env.py` and `commands/schema.py`, which workshops.md names by path.
  - I read these to answer the exit-code and invocation questions.
- **Unverified:**
  - Whether edits to the mounted `./automation` apply without restarting `prefect-flows`. The Dockerfile comment says attendees can edit live, but `serve.py` registers once.
  - Whether `PREFECT_API_URL` as read by `cycle` (no `/api` suffix, default `http://localhost:4200`) matches the host environment. This is the possible bug already listed in workshops.md.
- **Smoke timing:** I did not add up the worst-case duration. The job limit is 25 minutes, and five readiness gates can each use about 5 minutes alone.

DECISION NEEDED: Should the Rust smoke run copy the Python behaviour (the alert wait and the four `try-it` waits never fail the job), or fail when a wait times out? Default: copy the steps, waits and timeouts exactly, but exit non-zero when an alert wait or a Loki wait times out. Record it as a departure from the Python tool that you accept.
DECISION NEEDED: Is the daily smoke run of both workshops the acceptance test for the first version? Default: yes, with the same 25-minute job limit and the same matrix.
DECISION NEEDED: When the two workshops become one command set, which `cycle` view wins: packt's grouped subflow view or autocon5's flat list? Default: packt's grouped view for both, because it is a superset.
DECISION NEEDED: Which evidence log query should be the shared default: autocon5's regex on `device`, or packt's `device` plus `peer_address` labels? Default: keep each as a per-workshop setting, since the label sets of the two logs may differ and I did not check.
DECISION NEEDED: Should the Sonda screen offer only `kind: runnable` files to post, and treat composable packs as read-only? Default: yes.
DECISION NEEDED: Accept that I read `_console.py`, `lifecycle/setup.py` and `lifecycle/compose.py`, which no named source gave by path? Default: accept and keep the findings that rest on them (the exit-code finding depends on `_console.py`).

CONFLICT: docs/discovery/sonda.md and reference/sonda/sonda-server/src/routes/scenarios.rs
