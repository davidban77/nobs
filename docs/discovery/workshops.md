# Discovery: workshops

Read on 2026-11-04 by a discovery session (read-only) from the copy under `reference/`. Facts come from reading the code and the documents; nothing was run.

# What the workshops repository needs from a lab tool

**Result:** the Python `nobs` is a host-side command line tool. It runs `docker compose` for a workshop and queries the running services over HTTP. It has 3 root commands and 25 commands per workshop. The two workshops (`autocon5` and `packt`) share one compose layout and the same 12 extra commands, which are near-copies. The Prefect flows, webhook and AI step run in Python containers, not in the tool.

## Rules

netpanda-ai-workbench rules, version 0.12.0, file workbench-00-about.md

- Every other rule file and `.claude/profiles/software.md` show 0.12.0, so the versions agree.
- All 10 sections are visible.
- The home-folder `CLAUDE.md` and `orchestration.md` name agents and a verdict scheme that differ from the workbench rules. This task is read-only, so I applied neither.

## Known facts

### How the tool is built

| Fact | Source |
|---|---|
| The tool is `nobs` 0.1.0, built with Typer and Rich. Its console script is `nobs = nobs.main:app`. | `reference/workshops/packages/nobs/pyproject.toml` |
| The repo is a uv workspace with members `packages/*` and `workshops/*`. It needs Python 3.11 or newer. | `reference/workshops/pyproject.toml` |
| Its dependencies are typer, infrahub-sdk[ctl], requests, httpx, pydantic, python-dotenv, pyyaml and psutil, plus the two workshop plugins. | `reference/workshops/packages/nobs/pyproject.toml` |
| Importing `nobs` imports `autocon5_workshop` and `packt_workshop`. Each plugin registers itself. Entry-point discovery is only a comment ("added later"). | `reference/workshops/packages/nobs/src/nobs/__init__.py` |

### How a workshop registers

A plugin builds one `Workshop` object and calls `register()` at import time (`reference/workshops/packages/nobs/src/nobs/workshops.py`).

| Field | Rule |
|---|---|
| `name` | Must match `^[a-z][a-z0-9-]{1,30}$`. It is also the compose project name. |
| `title` | Free text. |
| `dir` | Must exist. |
| `compose_file` | Optional. Default is `dir/docker-compose.yml`. |
| `bootstrap` | Optional hook. Both plugins copy `.env.example` to `.env` if `.env` is missing. |
| `extra_commands` | List of functions. Names may not collide with reserved names or repeat. |
| `capabilities` | Subset of `status`, `alerts`, `maintenance`, `schema`. The default is all four. |

- A duplicate workshop name raises an error.
- Both plugins register the same 12 extra commands: `load-infrahub`, `evidence`, `rca`, `cycle`, `silences`, `try-it`, `flap-interface`, `incident`, `page`, `reset`, `scenarios`, `preflight`.
- The plugins are `reference/workshops/workshops/autocon5/src/autocon5_workshop/__init__.py` and `reference/workshops/workshops/packt/src/packt_workshop/__init__.py`.

### Commands

The surface is built in `reference/workshops/packages/nobs/src/nobs/main.py`.

**Root commands** work from any folder.

| Command | Inputs | What it does |
|---|---|---|
| `nobs setup` | none | Needs `uv`. Runs `uv sync --all-packages`, then preflight, then every workshop's bootstrap. Exit 1 if `uv` is missing, the sync fails or preflight fails. |
| `nobs preflight` | none | Checks docker on PATH and Compose v2. RAM: 8 GiB or more is ok, 6 to 8 warns, under 6 fails. Free disk in the current folder: 5 GiB or more, else fail. Sends a HEAD request to ghcr.io, registry-1.docker.io, github.com, registry.opsmill.io and grafana.com. Network problems only warn. |
| `nobs workshops` | none | Prints the registry as a tree. |

**Per-workshop commands:** `nobs <workshop> <command>`, or just `nobs <command>` from inside the workshop folder. In that case the root primitives (`setup`, `preflight`, `workshops`) stay as they are.

| Group | Command | Inputs |
|---|---|---|
| Lifecycle | `setup` | none |
| | `up` | `--build/--no-build` (default `--no-build`), `[SERVICES...]` |
| | `down` | none (keeps volumes) |
| | `destroy` | none (removes volumes and orphans) |
| | `restart` | `[SERVICES...]`. With none: `down`, then `up -d --build`. |
| | `ps` | none |
| | `logs` | `[SERVICE]`, `--tail` (200), `-f/--follow` (default on) |
| | `exec` | `SERVICE [COMMAND...]` (default `sh`) |
| | `build` | `[SERVICES...]`, then `up -d` |
| Capabilities | `status` | `--prom-url`, `--loki-url`, `--am-url`, `--infrahub-url`, `--grafana-url`, `--prefect-url`, `--sonda-url`. Exit 1 if any service is down. |
| | `alerts` | `--am-url`, `-l/--label key=value` (repeatable), `--silenced/--no-silenced`, `--inhibited/--no-inhibited`. Exit 1 if Alertmanager is unreachable, 2 for a bad label. |
| | `maintenance` | `-d/--device` (required), `--state/--clear`, `--kind` (default `WorkshopDevice`), `--address`, `--token`, `--loki-url`. Needs the token. |
| | `schema load` | `PATH`, `--address`, `--token`. Runs the `infrahubctl schema load` program. |
| Extras | `load-infrahub` | `--schema`, `--lab-vars`, `--address`, `--token`, `--skip-schema` |
| | `evidence` | `DEVICE PEER`, `--afi-safi`, `--instance`, `--log-minutes`, `--log-limit`, `--prom-url`, `--loki-url`, `--infrahub-url`, `--token` |
| | `rca` | `[DEVICE] [PEER]`, `--last`, `--minutes`, `--loki-url` |
| | `cycle` | `DEVICE PEER`, `--trigger`, `--status firing\|resolved`, `--minutes`, `--am-url`, `--loki-url`, `--prefect-url`, `--webhook-url` |
| | `silences` | `--show-expired`, `--am-url` |
| | `try-it` | `--auto`, five URL options, `--token` |
| | `flap-interface` | `-d` (srl1), `-i` (ethernet-1/1), `--duration` (4m), `--up-duration` (30s), `--down-duration` (60s), `--cascade-delay` (10s), `--no-cascade`, `--sonda-url`, `--loki-url`, `--sonda-api-key`, `--follow` |
| | `incident` | `-d`, `--primary-interface` (ethernet-1/10), `--backup-interface` (ethernet-1/1), `-k` (only `link-failover`), `--duration` (3m), `--sonda-url`, `--prom-url`, `--sonda-api-key`, `--follow` |
| | `page` | `-a` (BgpSessionNotUp), `-d`, `-p`, `--alertmanager-url`, `--now` |
| | `reset` | `--alertmanager-url`, `--sonda-url`, `--loki-url`, `--infrahub-url`, `--skip-sonda-logs` |
| | `scenarios` | `--sonda-url` |
| | `preflight` | `--skip-c/--with-c`, `--out-dir` (default `/tmp/preflight-out`) |

- Behaviour of some commands:
  - `reset` clears maintenance flags, re-runs the `sonda-setup` container and expires workshop silences.
  - `reset` also deletes the cascade scenarios and restarts `sonda-server` or `sonda-logs` if they look stuck.
  - `reset` runs `uv run nobs autocon5 ...` and `docker compose` as subprocesses.
- The per-workshop `preflight` has 3 layers:
  - Layer A waits for data in Prometheus and Loki.
  - Layer B queries every dashboard panel through Grafana's `/api/ds/query`.
  - Layer C takes headless-Chromium screenshots with Playwright. `--skip-c` skips it.
  - The three layer files are in `reference/workshops/workshops/autocon5/src/autocon5_workshop/preflight/`. They write `REPORT.md` and JSON files to `--out-dir`.
- Detailed sources for the table:
  - Lifecycle: `reference/workshops/packages/nobs/src/nobs/lifecycle/commands.py`.
  - Capabilities: `reference/workshops/packages/nobs/src/nobs/commands/`.
  - Extras: `reference/workshops/workshops/autocon5/src/autocon5_workshop/`.

### Services each workshop starts

Each workshop has 22 services, the same in both. The project name is the workshop name and the network is `workshop-<name>` (`reference/workshops/workshops/autocon5/docker-compose.yml`, `reference/workshops/workshops/packt/docker-compose.yml`).

| Group | Services (host port) |
|---|---|
| Sonda | `sonda-server` (8085, container 8080); `sonda-setup` (runs once, registers scenarios); `sonda-logs` |
| Collectors | `telegraf-srl1` (9005); `telegraf-srl2` (9006); `vector` (8686; 1514/udp inside the network) |
| Storage | `loki` (3001); `prometheus` (9090, remote-write receiver on) |
| Alerting and views | `alertmanager` (9093); `grafana` (3000) |
| Infrahub | `infrahub-db` (Neo4j); `infrahub-cache` (Redis); `infrahub-message-queue` (RabbitMQ); `infrahub-server` (8000); `infrahub-task-worker` |
| Automation | `webhook` (9997); `prefect-postgres`; `prefect-redis`; `prefect-server` (4200); `prefect-services`; `prefect-worker`; `prefect-flows` |

- Both workshops use the same fixed container names and host ports. `nobs <ws> up`, `build` and `restart` refuse to run if the other workshop's containers exist (`_guard_foreign_containers`). The check is `docker ps -a --filter label=com.docker.compose.project=<other>`.
- The `up` command prints a fixed URL panel with 7 URLs.

### How the tool talks to the services

All calls come from the host.

```
nobs (host) ──subprocess──► docker compose --project-name <ws> -f <ws>/docker-compose.yml
   │
   ├─HTTP─► Prometheus   /-/ready  /api/v1/query  /api/v1/alerts  /api/v1/query_range
   ├─HTTP─► Loki         /ready  /loki/api/v1/query_range  /loki/api/v1/series
   ├─HTTP─► Alertmanager /-/ready  GET /api/v2/alerts  GET+POST /api/v2/silences  DELETE /api/v2/silence/{id}
   ├─HTTP─► Grafana      /api/health  (preflight: /api/ds/query, /login, /api/search, /d-solo/…)
   ├─HTTP─► Infrahub     /health  POST /graphql (header X-INFRAHUB-KEY)  + infrahub-sdk + infrahubctl
   ├─HTTP─► Prefect      /api/health  POST /api/flow_runs/filter
   ├─HTTP─► webhook      POST /v1/api/webhook  (a fake Alertmanager payload)
   └─HTTP─► sonda        /health  GET+POST /scenarios  GET+DELETE /scenarios/{id}  POST /events
```

- Sources:
  - Clients: `reference/workshops/packages/nobs/src/nobs/clients/*.py`.
  - Other calls: `reference/workshops/packages/nobs/src/nobs/commands/status.py` and the extras.
- `annotate` posts an event to sonda's `/events`, with a Loki sink URL (`http://loki:3001`, the in-network address). Sonda forwards the event to Loki. The `maintenance` command uses this to write the "config push" log line.
- Inside the compose network:
  - Prometheus scrapes `telegraf-srl1:9005` and `telegraf-srl2:9005`. The telegraf containers scrape sonda's `/scenarios/metrics`.
  - Alertmanager sends `BgpSessionNotUp` and `PeerInterfaceFlapping` alerts to `http://webhook:9997/v1/api/webhook`.
  - The webhook calls Prefect's `run_deployment("alert-receiver/alert-receiver")`. That runs the Python flows in `prefect-flows`.
  - The flows query Prometheus, Loki, Alertmanager, Infrahub and sonda, then write decisions to Loki.
  - Grafana reads Prometheus, Loki and Infrahub's GraphQL (through the Infinity plugin).
  - Sources: `reference/workshops/workshops/autocon5/prometheus/prometheus.yml`, `.../alertmanager/alertmanager.yml`, `.../grafana/datasources.yml`, `.../webhook/app/api.py`, `.../automation/serve.py`.
- `incident` does not use Telegraf. It sends metrics straight to Prometheus by remote-write (`reference/workshops/workshops/autocon5/src/autocon5_workshop/incident.py`).
- `flap-interface` and `incident` build a sonda v2 scenario body in memory (`version: 2`, `while:` and `after:` clauses) and POST it. They use device-specific metric names. `srl1` uses gNMI names such as `srl_bgp_oper_state`. `srl2` uses SNMP names such as `bgpPeerState`. `flap_topology.py` reads `lab_vars.yml` to find peers.

### Environment variables and files

Every variable and file below comes from `reference/workshops/workshops/autocon5/.env.example`, `.../docker-compose.yml` and the command files.

| Item | Detail |
|---|---|
| `workshops/<ws>/.env` | Copied from `.env.example` by bootstrap. `nobs` merges it into the process environment at startup (`reference/workshops/packages/nobs/src/nobs/lifecycle/env.py`). Process environment wins. Compose also loads it by itself. |
| Service URLs | `PROMETHEUS_URL`, `LOKI_URL`, `ALERTMANAGER_URL`, `INFRAHUB_ADDRESS`, `INFRAHUB_API_TOKEN`. Defaults are `http://localhost:` plus 9090, 3001, 9093, 8000. |
| Sonda | `SONDA_SERVER_URL` (`http://localhost:8085`), `SONDA_API_KEY` (empty means no auth), `SONDA_LOKI_SINK_URL` (`http://loki:3001`), `SONDA_PROM_REMOTE_WRITE_URL` (used by `incident`; it is not in `.env.example`). |
| Other URLs | `PREFECT_API_URL` and `WEBHOOK_URL` (read by `cycle`; the webhook default is `http://localhost:9997/v1/api/webhook`). |
| Grafana | `GRAFANA_USER`, `GRAFANA_PASSWORD` (both `admin`), `GRAFANA_URL`, `GRAFANA_URL_LAYER_C` (preflight only). |
| AI step | `ENABLE_AI_RCA` (false), `AI_RCA_PROVIDER` (`openai`, `anthropic` or `demo`), `AI_RCA_MODEL`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`. These are read by the `prefect-flows` container, not by the CLI. |
| Image pins | `SONDA_IMAGE`, `PROMETHEUS_IMAGE`, `LOKI_IMAGE`, `GRAFANA_IMAGE`, `ALERTMANAGER_IMAGE`, `TELEGRAF_IMAGE`, `INFRAHUB_IMAGE`, `VECTOR_IMAGE`. |
| Preflight | `PREFLIGHT_OUT_DIR`, `PREFLIGHT_WORKSHOP_DIR`. |
| Host rewrite | If `INFRAHUB_ADDRESS` contains `infrahub-server`, host-side commands use `http://localhost:8000`. The rewrite happens at each call, not in the process environment. This keeps the value correct for compose (`reference/workshops/packages/nobs/src/nobs/lifecycle/env.py`). |
| Workshop files the CLI reads | `docker-compose.yml`, `.env`, `lab_vars.yml` (devices, interfaces, intended BGP peers), `infrahub/schema.yml` (`WorkshopDevice`, `WorkshopInterface`, `WorkshopBgpSession`), `grafana/dashboards/*.json` (preflight). |
| Seed data | `lab_vars.yml` defines `srl1` (AS 65101) and `srl2` (AS 65102), two healthy links and one intentionally broken peer each. |
| Alert rules | `BgpSessionNotUp` and `InterfaceAdminUpOperDown` (Prometheus), `PeerInterfaceFlapping` (Loki). |

### What an attendee types

The sources are the commands found in each workshop's `guides/` folder.

- **Before the day:**
  - `uv sync --all-packages`
  - `nobs setup` or `nobs preflight`
  - `nobs <ws> up`
  - `nobs <ws> status` (repeat until all rows say ok)
  - `nobs <ws> load-infrahub`
  - `nobs <ws> down`
- **Start of each part:** `nobs <ws> reset`, then `status`.
- **Part 1:** `flap-interface --device srl1 --interface ethernet-1/1` (and `ethernet-1/10`), `destroy` then `up` for a clean slate.
- **Part 2:** `flap-interface` for srl1 and srl2, `alerts`, `restart grafana`.
- **Part 3:** `cycle srl1 10.1.99.2`, `cycle … --trigger`, `evidence srl1 10.1.99.2`, `rca srl1 10.1.99.2`, `maintenance --device srl1 --state` and `--clear`, `try-it --auto`, `logs prefect-flows`, `flap-interface … --no-cascade`, `up` after editing `.env`.
- **Advanced (autocon5 only) and take-home (packt):** `incident --device srl1`, `incident --help`, `alerts`, `reset`.
- **Tools used with the CLI:** Grafana, Prometheus, Alertmanager, Prefect and Infrahub web pages in a browser.
- Differences between the two workshops:
  - `autocon5` is 4 hours in person. Part 2 is hands-on and there is an `advanced-cascades.md` guide.
  - `packt` is 3 hours online. Part 2 is a guided demo and there is a `take-home.md` guide.
  - Sources: `reference/workshops/workshops/autocon5/README.md` and `reference/workshops/workshops/packt/README.md`.

### What is tested

I read the first 80 lines of `test_flap_cascade.py` and the first 60 lines of `test_reset_infrahub_ensure.py`. I read the other seven test files in full. All nine are under `reference/workshops/tests/` and use pytest with mocks.

| Test file | What it covers |
|---|---|
| `test_workshops_model.py` | `Workshop` validators and `register()`. |
| `test_main_detect.py` | Detecting the workshop from the current folder. |
| `test_env_loader.py` | `.env` merging and the Infrahub address rewrite. |
| `test_compose_env.py` | The compose subprocess gets the unrewritten Infrahub address. |
| `test_up_collision_guard.py` | The foreign-container guard on `up`, `build` and `restart`. |
| `test_registry.py` | Each plugin shows up in `nobs workshops`. |
| `test_flap_cascade.py` | Cascade body builders, for both plugins. |
| `test_reset_infrahub_ensure.py` | The probe-then-load step of `reset`, for both plugins. |
| `test_packt_evidence_logs.py` | The packt evidence log query. |

- No test touches `status`, `alerts`, `maintenance`, `schema load`, the HTTP clients, `preflight`, `setup`, `cycle`, `rca`, `silences`, `incident`, `page` or `try-it`.
- CI is `reference/workshops/.github/workflows/validate.yml`:
  - A lint job on every PR validates YAML, TOML, Python syntax and shell syntax. It also runs `docker compose config` for both workshops, ruff and pytest. It checks that compose bind-mount sources exist.
  - A smoke job runs on a schedule or by hand, once per workshop on its own runner. It runs `setup`, `up`, waits for readiness, runs `load-infrahub`, waits for a firing `BgpSessionNotUp`, runs `try-it --auto`, then `destroy`.
- The 3-layer preflight (`nobs <ws> preflight`) is a regression check for the dashboards and data, run by hand.

### Shared and workshop-specific

- **Shared:**
  - The `nobs` package and the 12 extra commands (same list and same code shape).
  - The compose layout (service names, ports, configs).
  - File names inside `sonda/catalog`, `prometheus`, `loki`, `alertmanager`, `grafana`, `telegraf`, `vector`, `infrahub`, `webhook`, `automation`.
- **Specific:**
  - Name and title.
  - The Prefect workflow label (`autocon5_quarantine_bgp` vs `packt_quarantine_bgp`, in `cycle.py` and `try_it.py`).
  - The compose project name hard-coded in `reset.py`.
  - Image versions (Prometheus v2.52.0 vs v3.14.0, Infrahub 1.1 vs 1.11.2, Loki 3.1.1 vs 3.6.16, sonda 1.22.2 vs 1.23.1, Python 3.12/3.10 vs 3.13/3.11 for `sonda-setup`/`webhook`).
  - The log query in `evidence.py` (packt matches `device` and `peer_address` labels; autocon5 uses a regex).
  - The guides and the README text.
  - The device names `srl1` and `srl2` and the broken peers `10.1.99.2` and `10.1.11.1`. These are hard-coded in `flap.py`, `flap_topology.py`, `try_it.py` and `reset.py`.

### Docs lead (file names only)

- Pages: `reference/workshops/docs/index.md`, `install.md`, `quickstart.md`, `workshop/{advanced,guides,index,part-1,part-2,part-3,tour}.md`.
- Also under `reference/workshops/docs/assets/`: one architecture SVG, a book cover and 30-odd screenshots. I did not open them.

## Reasoned conclusions

1. **The tool is a thin host-side layer.** It runs `docker compose`, makes HTTP calls and prints tables. It holds no state. This rests on `main.py`, the lifecycle code, the clients and the command files.
2. **The Python in the containers is not the CLI.** The webhook, Prefect flows and AI step run inside `prefect-flows` and `webhook`. The CLI only starts them, looks at them and triggers them. A Rust `nobs` can work without touching them. This rests on the compose files, `serve.py`, `api.py` and the env table. I did not read `flows.py`, so I do not know what else the flows need from the host.
3. **The packt plugin is a copy of the autocon5 plugin with renames.** Evidence: the same 12 commands, no "autocon5" string in the packt `src`, and a short list of differences (workflow name, project name, evidence query). I did not diff the files, so this is likely but not proven.
4. **Three things can only be done by calling Python libraries today.**
   - The Infrahub SDK is used for `filters`, `create` and `save`.
   - `infrahubctl schema load` is run as a subprocess.
   - Playwright is used for preflight Layer C.
   - CI also shows a direct `POST /api/schema/load` and GraphQL with `X-INFRAHUB-KEY`, so a plain HTTP route exists for the schema step. This rests on `commands/schema.py`, `maintenance.py`, `load.py`, `layer_c.py`, `validate.yml` and `infrahub/schema.yml`.
5. **Attendees depend mostly on 12 commands.** These are `up`, `down`, `destroy`, `restart`, `logs`, `status`, `alerts`, `load-infrahub`, `reset`, `flap-interface`, `cycle`, `evidence`, `maintenance`, `rca`, `try-it` and `incident`. That is 16 names, but the others (`exec`, `ps`, `build`, `silences`, `page`, `scenarios`, `schema load`, `setup`, `workshops`, `preflight`) are operator tools. This rests on the guides grep.
6. **Test coverage of the host tool is thin.** Only lifecycle, env, registry and the cascade builders are covered by pytest. The rest is checked only by the scheduled smoke run (`try-it --auto`). A Rust port has little to port tests from; the smoke run is the main behaviour reference.
7. **Several host-side behaviours depend on the repo layout.**
   - `reset.py` computes `_repo_root()` as four parent folders up.
   - `reset.py` runs `uv run nobs autocon5 ...`.
   - The workshop folder is found because the plugin is installed from inside the repo.
   - A tool used "in all repositories" and in cloud sessions has no such guarantee.

## Open questions

- **Not read.** The named folder `reference/workshops/workshops` has 158 files. I did not read these in full:
  - Prefect and workflow code: `automation/flows.py`, `flows.Dockerfile`, `requirements.txt`, and `workshop_sdk.py` beyond its first 150 lines (autocon5 only; packt's copy not opened).
  - Webhook: `main.py`, `pyproject.toml`, Dockerfile.
  - Telegraf and Vector: configs and Dockerfile.
  - Prometheus and Loki: `loki-config.yml`, recording rules, packt rules.
  - Alertmanager: packt's `alertmanager.yml`.
  - Grafana: dashboards JSON, `grafana.ini`, Dockerfile.
  - Sonda: all 11 catalog files, packt's setup script.
  - Infrahub: `infrahub/README.md`.
  - Packt data: its `lab_vars.yml`, schema and `pyproject.toml`.
  - Docs: all `docs/*.md` except `docs/env-lifecycle.md` (autocon5).
  - Guides: I only searched them for `nobs` command lines.
  - Packt plugin code: most files, except `__init__.py` and a search of `src` for the autocon5 name and workflow label.
- **Reading outside the named sources.** I also read `reference/workshops/tests/*`, `reference/workshops/.github/workflows/validate.yml` and `reference/workshops/.env.example`. No named source gives their paths. By the skill they are leads, and I followed them. The "what is tested" answer depends on them.
- **Leads not followed:** `reference/workshops/docs-packt/` and `reference/workshops/.github/workflows/docs.yml`.
- **Version of the sources:** only `nobs` 0.1.0 is stated. I did not read the commit, so I cannot say which commit the copy is.
- **Possible bug, not confirmed:** `cycle` reads `PREFECT_API_URL` as the Prefect base URL and appends `/api/flow_runs/filter` (`reference/workshops/workshops/autocon5/src/autocon5_workshop/cycle.py`). Prefect's own setting normally ends in `/api`. I did not test whether a host with that variable set would double the path.
- **Mismatches in text:** `reference/workshops/packages/nobs/README.md` shows `nobs status`, `nobs alerts`, `nobs schema load` and `nobs maintenance` as root commands. `main.py` has them only per workshop, or at root when run from inside a workshop folder. The `lab_vars.yml` header says it populates Nautobot, but the code loads Infrahub. A comment in `prometheus.yml` says `telegraf-srl1` receives remote-write, but compose has Telegraf scraping sonda. `reference/workshops/pyproject.toml` mentions `workshops/autocon5/scripts/load_infrahub.py`, which does not exist in the list.
- **Not verified:** `AlertmanagerClient.create_silence` exists, but I did not find a CLI command that calls it. I did not search for it.

DECISION NEEDED: Accept that I read `reference/workshops/tests/*`, `.github/workflows/validate.yml` and `.env.example`, which the sources did not name? Default: accept and keep the "what is tested" findings.
DECISION NEEDED: Is the first version only the host-side CLI, leaving the compose stack and the Python containers (webhook, Prefect flows, AI step) as they are? Default: yes, host-side only.
DECISION NEEDED: Does nobs take in all 12 workshop commands and the 3 root commands in the first version, or only the ones attendees type (the 16 in conclusion 5)? Default: all, because requirements say it "takes in the lab tooling".
DECISION NEEDED: Is the per-workshop `preflight` in the first version (Layer A and B, Layer C with Playwright)? Default: include Layers A and B; leave Layer C out.
DECISION NEEDED: Is the Python `nobs` in `reference/workshops` replaced or kept (requirements: not decided)? Default: keep it until the Rust one matches it on the `try-it --auto` smoke run, then replace it.
DECISION NEEDED: How does nobs find a workshop when run from any repository or a cloud session: from a manifest file in the workshop folder, or built into the program? Today it is Python code in each plugin and `_repo_root()` guesses. Default: a file in the workshop folder; nothing else is known to decide it.
DECISION NEEDED: For Infrahub (schema load, device upsert, maintenance flag), call Infrahub's HTTP API (`/api/schema/load`, `/graphql`) directly, or run `infrahubctl`? Default: HTTP API directly, because a Rust program has no Infrahub SDK.
DECISION NEEDED: Are `autocon5` and `packt` kept as two separate workshops with duplicated code, or as one set of commands with per-workshop settings (name, workflow label, image pins)? Default: one set of commands; the differences found are few. This needs the files I did not read checked first.
DECISION NEEDED: Which workshop behaviours must stay exactly as they are, given that the guides print expected outputs? Default: the command names, flags and exit codes in the table above.

CONFLICT: reference/workshops/workshops/autocon5/docs/env-lifecycle.md and reference/workshops/packages/nobs/src/nobs/lifecycle/env.py

(`env-lifecycle.md` says `load_env` rewrites `INFRAHUB_ADDRESS` to localhost and re-exports it to the process environment. `env.py` explicitly does not, and two tests pin that. The code and tests favour `env.py`. The same document also lists `AI_RCA_PROVIDER` as `openai` or `anthropic`, but `.env.example` also allows `demo`.)
