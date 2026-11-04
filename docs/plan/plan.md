
# Plan: nobs, first version

Date: 2026-11-04. Owner of the plan: architect. Status: draft.

## 1. Outcome

When the first version is done, one Rust program, `nobs`, replaces three Python tools:
- `labcli`: David's command line for containerlab and docker compose labs.
- `netobs`: the command line of the lab for the book "Modern Network Observability".
- The Python `nobs`: the small tool inside the workshops repository.

`nobs` keeps the command names, flags and exit codes that attendees and scripts type today. It also talks to Sonda over HTTP. Sonda is David's synthetic telemetry generator; it has an HTTP server, `sonda-server`, that loads and runs scenario files. A Ratatui screen sits on the same code and shows one lab system at a time, with the Sonda view first. Ratatui is a Rust library for terminal screens.

containerlab builds virtual network labs from a topology file, using containers. It cannot run natively on macOS. On David's Mac it runs inside `clab`, an existing Linux machine (Ubuntu, arm64) made with OrbStack. OrbStack is the Mac program that runs Docker and Linux machines. The lab nodes are Nokia SR Linux, a network operating system that runs as a container and works on arm64. The Arista cEOS labs (cEOS is Arista's network operating system as a container image) wait for the Linux host that David will supply later.

`nobs` is built for Mac and Linux. The static Linux build is what a cloud session would run. The first version is not checked inside a cloud session: the check comes with the Linux host. David's goal, to use `nobs` locally and in cloud sessions, stays the goal.

The workshops switch from the Python `nobs` to the Rust one only after the Rust one passes the smoke run (S1). The switch itself is not part of this plan.

Success checks. A person can watch each one run:

- S1. Smoke, and it can fail. A script in this repository (`scripts/smoke.sh`) runs the workshops' smoke sequence with the Rust `nobs`, once for `autocon5` and once for `packt` (two workshops in the workshops repository): `setup`, `up`, five readiness waits, `load-infrahub`, wait for a firing `BgpSessionNotUp` alert, `try-it --auto`, `destroy`. The steps, waits and timeouts are copied from `reference/workshops/.github/workflows/validate.yml` (see `docs/discovery/workshops-smoke.md`, section 1). Each workshop run stays within 25 minutes. The script runs on a copy of the workshops folders outside `reference/`, with `nobs.toml` files added. It passes on the Mac (OrbStack's Docker) and on a Linux host (the `clab` machine, or David's host when it exists). The Mac run is native arm64: the architect checked on 2026-11-04 that every published image of both workshops has an arm64 build, except packt's Telegraf 1.39.3, which is unchecked. The five local builds are confirmed by the first real `up` (step 7). If a run meets an amd64-only image, the run stops and David is asked; S1 is not weakened. Four runs prove that it can fail, and each names the wait or check it trips:
  - Alert wait. After all five readiness waits pass and before the alert wait starts, stop the service that the alert wait polls. Step 16 names that service from `validate.yml`. The script must exit non-zero at the alert wait and name it.
  - Loki wait. Stop the `webhook` container after `up`. The alert still fires, the Prefect flow does not run, and the Loki waits in `try-it --auto` time out. The script must exit non-zero and name the Loki wait. Step 16 confirms this by running it.
  - Guard, containers. With a container of the same project name already present, the script refuses to start, exits non-zero and leaves that container untouched.
  - Guard, ports. With a throwaway listener on one of the host ports the compose file publishes, the script refuses to start, exits non-zero and changes nothing.

  The Python tool exits 0 in the alert-wait and Loki-wait cases.
- S2. Same exit code. One named command from each old tool runs with the old and the new program on the same fixture, and the exit codes are equal on success.
  - labcli: `docker ps` against the labcli example compose file (step 2).
  - netobs: `docker ps -s ch5` (step 17, where the `chapters/` lookup is built).
  - workshops: `nobs autocon5 status`, and `alerts` with a bad label, which exits 2 (step 9).
  - The old tools run from a copy outside `reference/`, because `uv` writes a virtual environment.
  - The deliberate difference: on failure, `nobs` returns the child program's exit code. labcli and netobs returned 0. The workshops' tool returns 1 or 2 from `status`, `alerts` and `setup`, but prints `fail` and exits 0 in `try-it`.
  - Step 27 runs all three again at the end.
- S3. Surface is kept. A test in this repository checks the command and flag inventory from the discovery tables: labcli 16 commands, the netobs commands, the workshops' 3 root commands and 25 per-workshop commands (9 lifecycle, 4 capability, 12 extra). `nobs --help` lists the groups and root commands. The test then checks each group's commands in `nobs <group> --help`, and the workshop commands in `nobs <workshop> --help` run inside a fixture workshop folder. It fails if one is missing. Excluded on purpose: the DigitalOcean `setup deploy|linux|destroy|show`, `load-prefect-secrets`, `delete-prefect-secrets`, every labby command, `DOCKER_COMPOSE_WITH_HASH`, labcli's README command `containerlab start` (it does not exist in the code), preflight layer C, and the `--skip-c` flag (accepted as a no-op). The test is created in step 2; each later step adds the commands it builds; step 27 compares the list with the full inventory.
- S4. containerlab on the Mac. Inside `clab`, the Linux build of `nobs` runs `containerlab deploy`, `inspect` and `destroy` for a two-node SR Linux lab created by this repository, under names that David's own labs do not use. The real nodes are started, listed and removed. The fixture topology uses an environment variable (a node label), the run uses `--sudo`, and `docker inspect` of a node container shows the label's value. A deploy that loses the variable under `sudo` fails this check.
- S5. Sonda on the Mac. Against a running `sonda-server` in OrbStack's Docker, once at 1.29.0 and once at 1.22.2 (the oldest version the workshops pin): load a local runnable file, list shows its id, `stats` shows counters, send one event to the stdout sink, delete it, list is empty. A duplicate load prints the server's 409 text. `show` prints what the server returns and no config object. If the 1.29.0 events endpoint does not accept the stdout sink, step 5 stops and reports to the requester. The supported Sonda range is 1.22 to 1.29.
- S6. Project file. A two-node SR Linux lab is built from the project file inside `clab`. A config is pushed over SSH to one node and read back. The same check on two cEOS nodes waits for the Linux host.
- S7. Screen. `nobs tui` opens the Sonda view and refreshes it every 1 to 2 seconds. Pressing `q` restores the terminal.
- S8. Checks. The six project checks in section 4 pass on the Mac, shown by command output. The Linux build is checked by running it (S4 and S9), because `clab` has no Rust toolchain.
- S9. Install. A release build of `nobs` for Linux arm64 (static) runs `nobs --version` in a plain Linux container with no Rust installed. The Linux x86_64 and macOS builds are built here. The x86_64 build is untested until David's Linux host exists.

## 2. Scope

In scope:
- The command line of all three tools, with the commands decided in `docs/plan/answers-2026-11-04.md`.
- The Sonda interface over HTTP. It shows only what the server returns. It reads one thing from a scenario file: the top-level `kind:` key (and `scenario_name` for display), so it offers only `kind: runnable` files to post. Sonda judges everything else in the file.
- containerlab with Nokia SR Linux nodes on the Mac. The cEOS labs on a Linux host later.
- A project file and build flow for containerlab labs.
- Config push over SSH, last, with the interface flap helper: SR Linux on the Mac if its CLI over SSH is enough, Arista EOS and Cisco IOS on the Linux host.
- A Ratatui screen on the same code, after the command line.
- A place in the design for a GNS3 provider.
- Reading a containerlab topology file as data, where a command takes `--topology` (the Nautobot loader). This is the one exception to "nobs parses only its own files".

Out of scope:
- Anything built for GNS3, labby's Telnet console bootstrap, and the DigitalOcean path. Labby is David's Python command line for GNS3 labs; it is the model for the design, not a feature list.
- Real cEOS runs on the Mac.
- Preflight layer C (Playwright).
- The Prefect secret commands, which stay as Python scripts in the lab.
- The container code of the workshops (`automation/flows.py`, `workshop_sdk.py`, the webhook). The settings in `nobs.toml` must match what the containers emit; steps 12, 14, 15 and 16 check the settings against the real stack (see section 3).
- A local scenario file check for Sonda, any dependency on `sonda-core`, and Sonda's gNMI and SNMP surface.
- Starting or stopping `sonda-server` from the Sonda interface.
- A library crate for other tools.
- Changing the Python `nobs`, the `reference/` folders, or the workshops, book lab and labcli repositories. No `nobs.toml` is added to the book lab or labcli repositories; tests use copies.
- Translating Infrahub schema files between versions.
- Aliases for `labcli` and `netobs`.
- Publishing anything, and a licence file, until David publishes.
- The detailed screen design.

## 3. What this plan rests on

The workshop stacks are made of: Prometheus (stores metrics), Loki (stores logs), Alertmanager (routes alerts), Grafana (dashboards), Prefect (runs automation flows) and Infrahub (a datastore for infrastructure data that the workshops fill with devices and BGP sessions).

| Statement | Known, inferred or missing | Source | Date checked |
|---|---|---|---|
| The rule files and the software profile are visible, all at version 0.12.0, all ten sections present. | known | `workbench-00-about.md` and the other rule files, `.claude/profiles/software.md` | 2026-11-04 |
| The old tools mostly run external programs (`docker compose`, `docker network`, `containerlab`) and make HTTP calls. They keep no state. | known (code reading) | `docs/discovery/labcli.md`, `lab.md`, `workshops.md` | 2026-11-04 |
| labcli and netobs never pass a failing exit code to the shell. The workshops' tool returns 1 or 2 from `status`, `alerts` and `setup`, but `try-it --auto` prints `fail` and exits 0 even when all four Loki waits time out. | known (code reading, nothing run) | `labcli.md` conclusion 3, `lab.md`, `workshops.md`, `workshops-smoke.md` section 1 | 2026-11-04 |
| `.env` precedence is decided: shell, then `.env`, then `.setup.env`. This differs from labcli and netobs, where `.env` beats the shell by accident. | known (code reading, decided) | `answers` round 1 | 2026-11-04 |
| The same command names exist in more than one tool with different defaults (see the defaults table in section 4). | known | `labcli.md`, `lab.md` | 2026-11-04 |
| containerlab cannot run natively on macOS. It runs inside a Linux arm64 machine. | known (documentation) | https://containerlab.dev/macos/ (via `docs/discovery/mac.md`) | 2026-11-04 |
| The Mac has the OrbStack Linux machine `clab` (Ubuntu arm64, containerlab 0.77.0, Docker 29.1.3, Compose 2.40.3, the SR Linux image pulled, 15 GiB visible, passwordless sudo, no Rust toolchain). It holds a stopped two-node SR Linux lab of David's. The Mac's files are visible there at the same paths. Its Docker engine is separate from OrbStack's own engine on the Mac side (29.8.2, Compose 5.5.0). | known (commands run by the architect on the Mac) | `answers` round 2 | 2026-11-04 |
| SR Linux is native on arm64, so two SR Linux nodes should deploy in `clab`. | inferred (documentation plus the existing stopped lab) | https://containerlab.dev/macos/, `answers` round 2 | 2026-11-04 |
| arm64 cEOS is not a usable path on the Mac. cEOS waits for the Linux host. | inferred (the sources conflict; the plan does not depend on either side) | `mac.md`, `answers` round 2 | 2026-11-04 |
| Memory is enough: 36 GiB on the Mac, no OrbStack limit, 15 GiB inside `clab`. | known | `answers` round 2 | 2026-11-04 |
| Every published image in both workshop compose files has an arm64 build (26 entries and the base images of the five local builds). | known (`docker manifest inspect`, run by the architect) | `answers` round 6 | 2026-11-04 |
| Packt's Telegraf 1.39.3, the five local builds (`telegraf-srl1`, `telegraf-srl2`, `grafana`, `webhook`, `prefect-flows`) and the Sonda 1.29.0 arm64 tag are not checked. | **missing** (step 5 checks the Sonda tag; the first real `up` in step 7 checks the rest) | `answers` round 6 | 2026-11-04 |
| The smoke job is a fixed sequence of 10 steps with five readiness gates, a 60-try 5-second alert wait, four Loki waits of 60 seconds, a 25-minute limit and a two-workshop matrix. | known (code reading, nothing run) | `workshops-smoke.md` section 1 | 2026-11-04 |
| Stopping `webhook` leaves the alert firing and trips the Loki waits, not the alert wait. | inferred (data flow); step 16 runs it | `workshops.md`, `workshops-smoke.md` | 2026-11-04 |
| The containers need only environment variables passed through compose and the `./automation` mount. No container calls the host tool. A rewritten `INFRAHUB_ADDRESS` must never be exported to the process environment, or compose injects it into `prefect-flows`. | known (code reading) | `workshops-smoke.md` section 2 | 2026-11-04 |
| Packt's plugin code is a rename-only copy of autocon5's in 11 of 12 compared files. `cycle.py` differs because packt's `flows.py` runs child flow runs. | known (read in full) | `docs/discovery/packt-infrahub.md` sections 1 and 2 | 2026-11-04 |
| Packt's `serve.py`, `requirements.txt`, Dockerfiles, `webhook/*`, `prometheus.yml`, `alertmanager.yml`, `pyproject.toml`, ten catalog files and `sonda-setup.sh` were compared only by line count and name search. | inferred (equal line counts, name hits only) | `packt-infrahub.md` open questions | 2026-11-04 |
| Those unread files get an owner: the discovery stage of step 7 reads `webhook/*`, `serve.py`, the Dockerfiles, `prometheus.yml`, `alertmanager.yml` and `pyproject.toml`; the discovery stage of step 14 reads the ten catalog files and `sonda-setup.sh`. | decided here | this plan | 2026-11-04 |
| The values that differ between the two workshops are the 11 in the settings list (D9). The device names, broken peers, label names, `incident` defaults and Infrahub kinds are identical in both. | known (read) for the files read; inferred for the files in the row above | `packt-infrahub.md` section 6, `answers` round 5 | 2026-11-04 |
| The Infrahub SDK sends three kinds of plain HTTP request: `GET /api/schema`, `POST /api/schema/load` (body `{"schemas": [...]}`) and GraphQL on `/graphql/<branch>`, with the `X-INFRAHUB-KEY` header. The docs give no HTTP detail, so the SDK source on the `stable` branch (the 1.23+ generation) stands in. | known (source), docs give nothing | `packt-infrahub.md` sections 3 and 4, `answers` round 5 (7) | 2026-11-04 |
| The exact GraphQL text for `filters`, the attribute and relationship payloads, whether the auth header and the calls differ between Infrahub 1.1 and 1.11.2, whether packt's `human_friendly_id` makes the wire payload differ (`hfid`), and the SDK version in the workshops' `uv.lock`. | **missing** (the capture at the start of step 10 settles it) | `packt-infrahub.md` conclusions 6 to 8 | 2026-11-04 |
| Infrahub 1.1 needs SDK 1.3.0 or newer (1.7.2 matches 1.1.10); 1.11 needs SDK 1.23.0 or newer. | known (documentation) | https://docs.infrahub.app/python-sdk/reference/compatibility | 2026-11-04 |
| The two schema files use different field names (`default_filter` and `display_labels` against `human_friendly_id` and `display_label`) and Infrahub rejects unknown fields. Each workshop's file is sent unchanged. | known (read); the rejection rule is documented, which version rejects which field is unknown | `packt-infrahub.md` sections 2 and 5 | 2026-11-04 |
| Sonda needs plain HTTP only. `GET /scenarios/{id}` returns `id`, `name`, `state`, `elapsed_secs`, `degraded`, a 9-field `stats` and `pending_ref` (only when `unresolved`), and no config. The fields of `PendingRef` and the keys of the 404 body are unread. | known, with a small **missing** part (step 5 reads `sonda-core` and `not_found`) | `workshops-smoke.md` section 5, `reference/sonda/sonda-server/src/routes/scenarios.rs` | 2026-11-04 |
| The workshop stacks pin Sonda 1.22.2 and 1.23.1. The workshops' `reset` is based on Sonda 1.13.1 behaviour, and the Sonda docs disagree with it about rows staying `paused`. | known (read); the true behaviour is unconfirmed | `workshops.md`, `sonda.md` | 2026-11-04 |
| The generic Sonda default is `http://localhost:8080`. The workshops use 8085 through `SONDA_SERVER_URL`. | known (decided) | `answers` (Sonda 4) | 2026-11-04 |
| Labby's shape is clear: Provider, Project, Node and Link classes, a project file with `main`, `nodes_spec`, `links_spec` and `vars`, and the flow topology, bootstrap, configs. Its SSH path hardcodes credentials, which must not be copied. | known (code reading) | `docs/discovery/labby.md` | 2026-11-04 |
| How a containerlab topology and SR Linux startup configs map onto labby's build flow. | **missing** (step 20 settles it) | containerlab documentation; `reference/labcli/example/containerlab/lab.clab.yml` is a lead | 2026-11-04 |
| The crates are chosen: clap (derive, env), serde-saphyr, toml, minijinja, ureq 3, russh with tokio inside the SSH module, ratatui 0.30 with crossterm 0.29. crossterm has had no release for 19 months. | known (crates.io), decided | `docs/discovery/crates.md`, `answers` round 2 | 2026-11-04 |
| clap nested groups, aliases and `env` flags work as the plan needs. | inferred (background knowledge); step 6 proves the workshop-name dispatch | `crates.md` | 2026-11-04 |
| russh may pull C crypto code that a static Linux cross build needs a C cross compiler for. | inferred risk | `crates.md` | 2026-11-04 |
| No mature Rust crate gives scrapli-style prompt handling. `rneter` is the closest and is young. | known (crates.io); fitness inferred | `crates.md` | 2026-11-04 |
| Whether SR Linux's CLI over SSH is enough for config push. | **missing** (step 24 settles it) | none yet | 2026-11-04 |

Every **missing** fact has a step that settles it, inside that step's discovery stage.

## 4. Work breakdown

Sizes are for one factory run. A factory run is one delivery of one agreed change: separate workers do the discovery, the plan, each task and the review, and the run ends with a report. S is up to about 500 lines with tests, M up to about 1,500, L up to about 2,500. Nothing is larger: a step that grows past its size is split before it is delivered.

Project checks. They are the same for every step, and every step runs them on the Mac. No task runner file exists yet.

1. `cargo build --workspace`
2. `cargo nextest run --workspace`
3. `cargo test --workspace --doc`
4. `cargo clippy --workspace --all-targets -- -D warnings`
5. `cargo fmt --all -- --check`
6. `cargo audit`

Real runs use the Mac's OrbStack Docker for Sonda and the workshop stacks, and `orb -m clab <command>` for everything that needs containerlab. Nothing runs inside `reference/`; copies go to a git-ignored scratch folder.

**Rule for every real run, in every step, on the Mac or inside `clab`.** A real run uses only fixtures and names that nobs or the step created: a copied folder, a unique project name, and for containerlab a lab name, node names and network name with a prefix (`nobs-t-`) that David's own labs do not use. Before it starts anything, the run checks for containers, networks, labs and published host ports with the same names. Some stacks pin their container names and host ports (the workshops and the book lab), and a unique project name does not avoid those; the check covers them, and the run does not rewrite the compose file to dodge a clash. If it finds anything it did not create, it stops and reports. It removes only what it created. David's stopped two-node lab in `clab` is never touched.

```
 1 shape -> 2 compose -> 3 Linux build -> 4 containerlab + Provider -> 5 Sonda CLI
   -> 6 locator + dispatch -> 7 workshop lifecycle + first real up
   -> 8 status -> 9 alerts, silences -> 10 Infrahub capture + fallback -> 11 Infrahub plain HTTP
   -> 12 cycle, evidence, rca, page -> 13 try-it -> 14 scenario builders -> 15 reset, preflight
   -> 16 smoke script (M1)
   -> 17-18 book lab -> 19 Nautobot -> 20 project file -> 21-23 screens -> 24 SSH push
   -> 25 flap helper -> 26 packaging -> 27 surface and exit-code check
```

| Step | Content | Proves |
|---|---|---|
| 1 (S) | **Shape.** A Cargo workspace with `nobs-core` (library, `publish = false`) and `nobs-cli` (the `nobs` binary, clap with derive). The core holds the Runner, a small piece of code that runs an external program from a list of arguments and returns its exit status, and a concrete compose type. No Provider trait yet. The binary has one command, `nobs docker ps` with `--compose` (default `./docker-compose.yml`). It builds `docker compose -f FILE ps --all`, as labcli does. A missing file exits 1. The child's exit code is passed back. Test fixture: a one-service compose file. No `.env` loading, no HTTP, no other dependency. The step lists which tools of the six checks are installed on the Mac (`cargo-nextest`, `cargo-audit`); a missing one is reported to the requester, not installed silently. | The layout builds and passes the six checks. The command line equals labcli's (a test on the argument list). A fake Runner returning 3 makes `nobs` exit 3, and the test fails if passthrough is removed. On the Mac, the fixture is started with `docker compose` under a unique project name and `nobs docker ps` shows it (real run). |
| 2 (M) | **Compose commands.** All labcli `docker` commands: build, pull, exec, debug, start, stop, restart, logs, ps, destroy, rm, update and network. `.env` and `.setup.env` load with the order shell, `.env`, `.setup.env`. Union flags `--compose` and `--scenario`. The netobs `docker` names are kept. `DOCKER_COMPOSE_WITH_HASH` is dropped. Proper `update`: pull the images of the selected services, then recreate them, and return the child's exit code; labcli's `--volumes` flag stays and keeps its old meaning (the step's discovery stage reads `labcli.md` conclusion 2 and `docker_utils.py:570-579` and confirms it). Proper `exec`: exactly one service, command default `bash`, a terminal attached when stdin is a terminal, otherwise `-T`; several services is a usage error (exit 2). Creates the S3 surface test. The labcli example is the test fixture. | Argument lists equal the labcli table, one test per command. Precedence tests fail if the order changes. Tests for `update`, `exec` and `network` (see the bug list). S2 for labcli. A real start, ps, logs, exec and destroy on the Mac, under a unique project name. |
| 3 (S) | **Linux arm64 build inside `clab`.** A static `aarch64-unknown-linux-musl` release build made on the Mac, run from its build folder inside `clab`. No installer: the file is visible at the same path. The build route is part of the step. If the Mac lacks the target or a linker, say what was added. A fallback is a native arm64 Linux build in a container on OrbStack. | `orb -m clab <path>/nobs --version` prints the version. `nobs docker ps` against the step 1 fixture lists a service started with `clab`'s own Docker under a unique project name. |
| 4 (M) | **containerlab provider, SR Linux first, and the Provider trait.** `containerlab deploy`, `destroy`, `inspect`, with `--sudo` and the exact command names. A two-node SR Linux fixture topology owned by this repository (image `ghcr.io/nokia/srlinux:latest`), with the lab name, node names and network name prefixed `nobs-t-`, and a label that reads an environment variable. Before deploying, the step lists the labs in `clab` and stops if the fixture name exists. A Provider is the code for one lab system (compose, containerlab, later GNS3). The trait is added here, drawn from the two real cases, compose `ps` and containerlab `inspect`; the step reports the shape it found, and does not add a method neither case needs. The `sudo` argument list keeps the environment (the step picks `-E` or `--preserve-env=VAR` and says why). The help text of `destroy` and `inspect` is corrected. | Argument lists with a fake Runner; the test fails if `--sudo` or the environment is dropped. A help-text test on the corrected strings. S4: the Linux build deploys, inspects and destroys the real two-node lab in `clab`, and `docker inspect` of a node shows the variable's value. |
| 5 (M) | **Sonda interface, command line.** The `nobs sonda` group: health, ready, list, show, stats, delete, load (a local file), event, metrics, version from `sonda_server_build_info`. Flag first, then `SONDA_SERVER_URL` and `SONDA_API_KEY`. Default `http://localhost:8080`. The key is never written to a file. `show` prints the fields the server returns and no config. `load` reads only the top-level `kind:` key of a YAML file. A file with `kind: composable`, a file with no top-level `kind:` key, and a JSON file are each refused with a message that names the reason, exit 1 (Sonda's catalog skips such files silently; `nobs` says why). The step reads `PendingRef` and the 404 body in `reference/sonda`. The build-info line is read with a few lines of own parsing, not a new crate. The step checks that the 1.29.0 and 1.22.2 arm64 images exist. | Tests against a stub HTTP server cover each endpoint, auth, and the 400, 401, 404, 409, 422, 429 and 502 text. Three refusal tests (composable, no `kind:`, JSON) fail if the check is removed. S5 on the Mac at 1.29.0 and 1.22.2, with a Sonda container started under a unique name. |
| 6 (M) | **Locator, settings and workshop-name dispatch.** The locator (rules below the table): `nobs.toml` for workshops (required) and labs (optional), lab folders found by layout, `--dir` and `NOBS_DIR`. The 11 per-workshop settings (D9). The clap fallback that treats an unknown first word as a workshop name and parses the rest with the workshop command set. Root command `workshops`. The located folder's `.env` is loaded; commands that need no located folder read `.env` from the current folder. | A workshop and a lab are found from any folder below them (tests on copies, including a book lab and a labcli lab with no `nobs.toml`). The labcli topology name is found as `lab.clab.yaml` and as `lab.clab.yml` (a test that fails if either is dropped). The commands that need a located folder exit 1 in a bare folder, and the ones that do not run there (a test per group). `nobs <ws> --help` in a fixture lists the workshop commands (S3). A wrong workshop name exits 2. The dispatch test fails if the fallback is removed. |
| 7 (M) | **Workshop lifecycle.** Root commands `setup` (preflight and bootstraps, no `uv sync`) and `preflight` (layers on the host). Lifecycle commands: setup, up, down, destroy, restart, ps, logs, exec, build. Bare `nobs <command>` inside a workshop folder. The foreign-container guard, extended by the real-run rule: `up` also refuses when containers or published ports of its own project exist that the run did not create. `up` has no `--build`, as today. The first real `up` of a copied `autocon5` stack on the Mac, with its five local builds, in OrbStack's Docker. The discovery stage reads `webhook/*`, `serve.py`, the Dockerfiles, `prometheus.yml`, `alertmanager.yml` and `pyproject.toml` of both workshops and records any difference from the 11 settings. | `up` refuses when the other workshop's containers exist, and when its own project's containers or ports exist (both guard tests fail if removed). With a fake Runner returning 3, each lifecycle command exits 3. The real `up` shows whether the local builds work on arm64; if one fails, the step reports which and stops. |
| 8 (M) | **Workshop service health.** The HTTP clients for Prometheus, Loki, Alertmanager, Grafana, Prefect, the webhook and Infrahub (a health probe), and the command `status`. Its Sonda probe uses the step 5 client. | `status` exits 1 when a service is down and 0 when all are up, against stub servers and then the real stack from step 7. |
| 9 (S) | **Alerts and silences.** `alerts` (exit 1 and 2 as today) and `silences`. | `alerts --label bad` exits 2. S2 for workshops. |
| 10 (M) | **Infrahub through the fallback, and the capture.** `load-infrahub`, `schema load`, `maintenance`. First task: capture the real requests (`filters`, `create`, `save`, `schema load`, and the auth header) from the live stacks of both Infrahub versions (1.1 with `autocon5`, 1.11.2 with `packt`), and record them in a file in the repository. The fallback is a small Python script in this repository, run with `uv run`, that makes the SDK calls for the device and interface upserts and the maintenance toggle; the schema load runs `infrahubctl schema load` through `uv run`. The script's SDK version is pinned per server to the row of the compatibility table that matches it. This step owns the script; step 11 deletes it. It is not the old Python `nobs`. Each workshop's `infrahub/schema.yml` is sent unchanged. The `INFRAHUB_ADDRESS` rewrite from `infrahub-server` to the host form is made per call and never exported. | The schema loads and the devices upsert twice in a row on both versions without error. A test fails if the rewrite is exported to the process environment. The capture is a file in the repository. |
| 11 (M) | **Infrahub, plain HTTP.** The same three commands call Infrahub directly: HTTP and GraphQL with the `X-INFRAHUB-KEY` header, built from the capture. The fallback script and the `uv run` calls are deleted when this client passes on both Infrahub versions. | The same real runs as step 10 pass on both versions with no `uv` and no `infrahubctl`, and the fallback script no longer exists. The stub test fails if the key header is dropped. A request-shape test compares the bodies with the capture. |
| 12 (M) | **Flow views.** `cycle`, `evidence`, `rca`, `page`. `cycle` groups child flow runs under their parent when they exist and shows a flat list otherwise, so one command serves both workshops. `evidence` takes its log query from the settings. The step checks the possible double `/api` path in the `PREFECT_API_URL` use. | `cycle` stubs for a flat and a grouped run show the right output (two tests). The two evidence queries differ (two tests). The PREFECT_API_URL test fails on a doubled path. On the real stack from step 7, `evidence` with the `evidence_query` setting returns log lines for a device. |
| 13 (M) | **`try-it`.** Including `--auto`. Exits non-zero when a Loki wait times out, in every mode (departure, section 6). | `try-it --auto` runs the same four paths as the smoke run against stubs, and exits non-zero when a wait times out (the test fails if that is removed). |
| 14 (M) | **Sonda scenario builders.** `flap-interface`, `incident`, `scenarios`. The builders build the v2 scenario body in memory and POST it. The `cascade_prefix` and `scenario_prefix` settings are used. The discovery stage reads the ten catalog files and `sonda-setup.sh` of both workshops. | The cascade bodies equal the Python bodies for the same input (pytest cases as reference). Run against Sonda 1.22.2 and 1.23.1, the versions the workshops pin. On the real stack, the scenario names the builders create match the `scenario_prefix` of the catalog files. |
| 15 (M) | **`reset`, and preflight A and B.** `reset` with the project name, silence hints and probe-then-load step from the settings. Preflight layers A and B per workshop. `preflight` accepts `--skip-c` as a no-op that prints that layer C is not built. | After `reset` on a stub, no scenario of the workshop is left running or paused (the test fails if `reset` skips a delete). On the real stack at Sonda 1.22.2, the scenario list is empty after `reset` and the silences matching `silence_hints` are gone. The `--skip-c` test fails if the flag is rejected or the note is missing. |
| 16 (M) | **Smoke script.** `scripts/smoke.sh` with the steps, waits and timeouts of the workflow, run on a copy of the workshops. It refuses to start when containers for the project already exist or the published host ports are busy; its exit trap is armed only after that check passes, so it removes only what the run created. It exits non-zero when the alert wait or a Loki wait times out, prints which wait failed, and dumps container logs on failure. It runs the two workshops one after the other. The discovery stage names the service the alert wait polls. | **Milestone M1: S1 passes on the Mac**, then on `clab`. The four failing runs of S1 (alert wait, Loki wait, guard on containers, guard on ports) each behave as written. M1 needs step 11 done, so the script needs no `uv`. |
| 17 (M) | **Book lab, core commands.** `lab deploy`, `destroy`, `purge`, `show`. Scenarios are listed from `chapters/`. The network is created first; before that, the run checks that the lab's fixed network, project name and node names are not already in use. The program stops on a failed step and names it. It waits for services. | A step failure stops the run and names the step (fake Runner). `purge` ignores only the missing-file case. `deploy` never prints success after a failed containerlab run. S2 for netobs (`docker ps -s ch5`). |
| 18 (M) | **Book lab, the rest.** `lab prepare`, `update`, `rebuild`. The cEOS image check, with the import instruction. Inside `clab`, one Docker engine serves containerlab and compose, so the shared network works. | In `clab`: the compose parts of a chapter against the shared network, with the SR Linux fixture. The real cEOS `lab deploy` waits for the Linux host. |
| 19 (M) | **Nautobot helpers.** `utils load-nautobot` (device type bug fixed, repeatable, reads the containerlab topology file as data), `delete-nautobot` (handles every page). | A second load changes nothing. A cEOS device is not created with the SR Linux type (fails on the old bug). A two-page stub is deleted completely. |
| 20 (L) | **Project file and build flow.** `nobs_project.yml` (labby's shape: main, nodes, links, vars), read with serde-saphyr, for containerlab. Templates with minijinja. The flow renders the topology and startup configs, then deploys. | A two-node SR Linux project renders the expected topology file (golden). In `clab`: it deploys under `nobs-t-` names. The YAML crate keeps `lab_vars.yml` values unchanged (a round-trip test). |
| 21 (M) | **Screen base and Sonda view.** `nobs-tui` crate (Ratatui 0.30, crossterm). It uses only `nobs-core` calls. A list of scenarios refreshed every 1 to 2 seconds, with the selected scenario's stats and the fields `show` returns. All six states the Sonda docs define are shown, and any other state text (the workshops' tests use `stopped`) is shown as the server sent it. Only runnable files can be posted. Packs are read-only. | S7. A test of the data layer without a terminal. A person watches it open and quit. |
| 22 (M) | **Screens: compose and containerlab.** The containerlab view runs from the Linux build inside `clab`. | A person drives each one; the CLI behaviour is unchanged (S2 and S3 still pass). |
| 23 (M) | **Screens: book lab and workshop.** | Same as step 22. |
| 24 (L) | **Config push over SSH.** Per-node config from a template plus variables. First task: a hands-on check of `rneter` against a real SR Linux node. Fallback: own prompt handling on russh. Tokio stays inside the SSH module. Credentials come from the project file or the environment, never hardcoded. Arista EOS and Cisco IOS wait for the Linux host. | S6 on the Mac (SR Linux). A wrong password is reported, not hidden. S9 is repeated, because russh may change the static build. |
| 25 (S) | **Interface flap helper.** `utils device-interface-flap`, over the SSH module. Credentials come from `NETWORK_AGENT_USER` and `NETWORK_AGENT_PASSWORD`. | Missing variables give a clear error and no connection attempt. A flap on the SR Linux node in `clab`. |
| 26 (S) | **Packaging check.** Release builds for macOS arm64 and Linux (x86_64 and aarch64, static), the Linux arm64 one checked in a plain Linux container. | S9. |
| 27 (S) | **Surface and exit-code check.** The S3 test against the full inventory and its exclusion list; the three S2 runs again. A file listing the places where the guides and READMEs differ from the code. | S2 and S3 pass together. |

The first change (step 1) is deliberately small. It proves the crate layout, the Runner seam, one real command with tests, and the six checks. It does not touch `.env`, HTTP, a trait, the screen or the Linux build.

### Defaults per tool

The old tools disagree. `nobs` keeps each default under the group that used it, and picks it from the folder it finds.

| Default | labcli lab | Book lab (netobs) | Workshop |
|---|---|---|---|
| Found by | `docker-compose.yml` in the folder (a `containerlab/` folder is needed only by the `containerlab` commands) | `chapters/` and `containerlab/lab.yml` | `nobs.toml` with `kind = "workshop"` |
| Topology file | `containerlab/lab.clab.yaml`, else `lab.clab.yml` | `containerlab/lab.yml` | none |
| Docker network | `labcli` | `network-observability`, subnet `198.51.100.0/24` | `network` setting (`workshop-<name>`) |
| `--sudo` default | on for `containerlab` | on for `containerlab`, off for `lab` | not used |
| Compose file | `./docker-compose.yml` (`--compose`) | `chapters/<scenario>/docker-compose.yml` | `docker-compose.yml` in the workshop folder |
| Compose project name | labcli's own | `netobs` | `project` setting |
| Scenario lookup | none | the folders under `chapters/` | none |
| `nobs.toml` | optional | optional | required |

Which commands need a located folder:

| Needs a located lab or workshop folder | Does not |
|---|---|
| `docker` without an explicit compose file; `containerlab` without an explicit topology file; `lab`; `utils`; the workshop commands | `sonda`, `tui`, `workshops`, and any command given an explicit file or `--dir` |

```
nobs <command>
  |-- needs no folder (right-hand column above) ----> run; .env read from the current folder
  '-- needs a folder:
        flag --dir or NOBS_DIR? ---------------------> use it
        else the nearest folder upwards that matches, in this order:
          1. nobs.toml (kind = workshop, or lab overrides)
          2. book lab layout      (chapters/ + containerlab/lab.yml)
          3. labcli lab layout    (docker-compose.yml)
        none matches -> exit 1, "no lab or workshop found from here"
        found -> its .env is loaded
```

An explicit flag beats the layout, and a `nobs.toml` value beats the layout default. `setup` is only the root command; the DigitalOcean `setup` group is out of scope.

### Bug list

Bugs the discovery reports found in the old tools are fixed in `nobs`, not copied. Each has one owning step and one test that fails if the fix is removed.

| Bug | Step | Test |
|---|---|---|
| labcli and netobs commands never pass the child's exit code; the workshops' `try-it` exits 0 on timeouts. | 1, 7, 13 | Fake Runner returning 3 gives exit 3 (docker group in 1, each lifecycle command in 7); a timed-out Loki wait gives non-zero (13). |
| `.env` beats the shell in labcli and netobs. | 2 | Precedence tests. |
| `docker update` always logs "update failed" and restarts nothing. | 2 | Argument list pulls then recreates; failure exit code is passed back. |
| `docker exec` with several services builds an invalid command; no `-T` handling. | 2 | One service accepted, several is exit 2; `-T` when stdin is not a terminal. |
| `docker network` requires `--name` for `ls` and `prune`; `create --driver` leaves a stray space. | 2 | `network ls` without `--name` builds a valid list; no stray argument. |
| labcli's default topology name (`.yaml`) differs from its README and example (`.yml`). | 6 | Both names are found in a labcli lab copy. |
| `sudo` may strip variables read by the topology. | 4 | S4 fixture label reaches the node container. |
| `containerlab destroy` logs "Deploying"; help examples say `show` and `netobs`. | 4, 2 | Help-text tests on the changed strings. |
| `lab deploy` prints "deployed" after a failed containerlab run; no step stops on failure. | 17 | Fake Runner failure stops the run and names the step. |
| `lab purge` ignores every failure, not only a missing file. | 17 | Only the missing-file exit is ignored. |
| `load-nautobot` creates every device with the SR Linux type; it is not repeatable. | 19 | Second load changes nothing; cEOS device type is cEOS. |
| `delete-nautobot` deletes only the first page. | 19 | Two-page stub is emptied. |
| The flap helper hardcodes credentials and ignores `NETWORK_AGENT_USER` and `NETWORK_AGENT_PASSWORD`. | 25 | Missing variables stop before any connection. |
| `cycle` may double the `/api` path from `PREFECT_API_URL` (possible, unconfirmed). | 12 | URL with and without `/api` gives one `/api`. |
| The workshops' `reset` assumes Sonda 1.13.1 behaviour. | 15 | Stub and real-stack assertions on the scenario list after `reset`. |
| The smoke job cannot fail on a timed-out wait. | 16 | The four failing runs of S1. |

One more finding is not a `nobs` bug: the book lab README and the workshops' READMEs name commands that differ from the code. `nobs` follows the code (S3), and step 27 writes the differences to a file for David.

## 5. Decisions

All decisions are **proposed** until the requester agrees.

**D1. Crate layout: a workspace with `nobs-core` and `nobs-cli`; `nobs-tui` is added at step 21.**
- The lead option is a workspace of three crates, where the core never prints or reads a terminal. The screen then reuses the core with no copying.
- The shortcut is one crate with `lib` and `main`. It saves about 30 lines of manifest, but nothing stops terminal code from leaking into the core.
- Two programs (`nobs` and `nobs-tui`) break "Rust, one program". Rejected.

```
   nobs-cli  (binary `nobs`)         nobs-tui  (Ratatui, step 21)
   clap commands, printing,          screens, keys, refresh
   exit codes
            \                          /
             v                        v
              nobs-core   (internal, publish = false, no printing)
       providers: compose | containerlab | (gns3: slot kept)
       runner:    runs external programs, returns the exit status
       clients:   sonda, prometheus, loki, alertmanager, infrahub, ...
       settings:  .env, nobs.toml, nobs_project.yml
```

**D2. Command line library: `clap` with derive and env.** A usage error exits 2, as Typer (the Python command line library the old tools use) does today. Risk: workshop names come from files at run time, so a fallback looks up an unknown first word as a workshop and parses the rest with the workshop command set. Step 6 proves it, early, before the lifecycle commands depend on it.

**D3. Running external programs: argument lists through a `Runner` trait, no shell.**

```
 command --> build the argument list (pure function) --> Runner
                                                          |-- real: std::process::Command, terminal inherited
                                                          '-- fake: records the list, returns a chosen exit code
```

Tests check the argument list and the exit code with no Docker running. A Docker API crate covers nothing we need, because compose and containerlab have no API.

**D4. Formats and crates.**
- TOML (`toml`) for nobs's own settings and the locator. YAML (`serde-saphyr`, with the `%YAML 1.2` output line turned off) for the project file and `lab_vars.yml`.
- nobs parses only its own files, plus a containerlab topology where `--topology` asks for it (Nautobot loader). Scenario files go to Sonda as they are; nobs reads only the top-level `kind:` key and `scenario_name`, so Sonda stays the judge of what a valid scenario is.
- Templates: `minijinja`, pinned to an exact version because it is on a fresh major version.
- Cost: two parsers, and Apache-2.0-only crates (minijinja, russh), which the answers accept.

**D5. Screen's place: a separate crate over the core, started with `nobs tui`.** Plain `nobs` prints help. No cargo feature switch for now. The core exposes data and polling calls, not printed text. Crossterm has had no release for 19 months; this is a risk to watch.

**D6. The core is synchronous.**
- HTTP is `ureq` 3. Threads and channels carry polling to the screen.
- Tokio is used only inside the SSH module (russh).
- Every new dependency is named in its change description.

**D7. Provider trait, added at step 4.** The trait is drawn from two real cases, compose `ps` and containerlab `inspect`, and starts with the one method both need, `show`. Step 1 has a concrete compose type and no trait.
- The lead option is this one: a trait made from one example is an abstraction for single use. Cost: step 1 has no provider seam to look at.
- The shortcut was the trait in step 1. It was cheaper to write, but the "provider" idea stayed unproven until step 4.
- A compose stack is services; a lab is nodes and links. If they do not fit one noun, step 4 reports it and the trait changes shape or is not added.
- The reviewer must refuse extra methods.

**D8. One binary, `nobs`, and no aliases.** Groups `docker`, `containerlab`, `lab`, `utils`, `sonda`, `tui`, `<workshop> <command>`, and the root commands `setup`, `preflight`, `workshops`. The old command names stay. Flags are the union. Defaults and the folder rules come from section 4. Exit codes are passed through; a usage error is 2. The README maps `labcli X` and `netobs X` to `nobs X`.

**D9. Locator file `nobs.toml`.** It sits in a workshop folder (required) or a lab folder (optional). The nearest file above the current folder wins. `--dir` and `NOBS_DIR` override. For workshops it holds the `kind` marker and exactly 11 settings; everything else stays in code, shared by both workshops.

```
Before:  Python code in each plugin; _repo_root() guesses four folders up
After:   workshops/packt/nobs.toml
           kind = "workshop"               (marker, not a setting)
           name            = "packt"
           title           = "<packt title>"
           project         = "packt"
           network         = "workshop-packt"
           workflow_label  = "packt_quarantine_bgp"
           cascade_prefix  = "packt-cascade-"
           scenario_prefix = "packt-"
           evidence_query  = "<packt's query>"
           silence_hints   = ["Workshop", "workshop"]
           schema_file     = "infrahub/schema.yml"
           [image_pins]    = <the compose pins that differ>
```

The device names `srl1`/`srl2`, the broken peers, the label names, the `incident` defaults and the Infrahub kinds are identical in both workshops, so they stay in code. If a third workshop differs, they move into the file. The settings must match what the containers emit. The alternative, a global list in `~/.config/nobs`, breaks in cloud sessions that start empty.

**D10. Sonda defaults differ on purpose.** `nobs sonda` uses `http://localhost:8080` when nothing else is set. Order: flag, then the shell, then the `.env` it reads, then the default. Inside a workshop folder, `sonda` reads the current folder's `.env`, which sets 8085 there; a `SONDA_SERVER_URL` in the shell still wins, as it does for every setting.

**D11. Test strategy.** A fake Runner for argument lists, a stub HTTP server for the clients, a surface test (S3), and real runs on the Mac under the rule in section 4. A test must fail when the behaviour it names is removed. Old pytest cases are references for the cascade builders. The old tools and the workshops run from copies outside `reference/`.

**D12. Comment discipline.** No comments by default, one short line at most (rule `workbench-80`).

**D13. How nobs reaches the `clab` machine: run a Linux arm64 build of nobs inside it.**

```
Mac (macOS arm64)
 |-- nobs (macOS build) ---------> OrbStack Docker (29.8.2): Sonda, workshop stacks
 '-- orb -m clab ----------------> machine clab (Ubuntu arm64)
       nobs (Linux arm64 build,    containerlab 0.77.0 + Docker 29.1.3:
       read from the same path)    SR Linux nodes, book lab network and compose
```

Before and after, for one command:

```
Option A (lead):  orb -m clab /Users/netpanda/projects/nobs/target/aarch64-unknown-linux-musl/release/nobs containerlab deploy --sudo
Option B:         nobs containerlab deploy --sudo      (macOS nobs adds the "orb -m clab" prefix itself)
```

- Option A is the lead. The program inside `clab` is the same program that runs on a Linux host, with no Mac-specific code. Cost: a second build (step 3, S) and a longer command; a shell alias covers that. Risk: a static cross build may need a linker or a container build, and russh (step 24) may complicate it.
- Option B is the shortcut. One Mac binary is easier to type. But it ties nobs to OrbStack, must translate paths and carry `sudo` and environment variables across `orb`, and the Mac behaves differently from Linux. It can come later as a small Runner variant, so A does not close it.
- The two Docker engines on the Mac do not share networks. A person must know which build talks to which engine. The README says so.

**D14. The smoke script copies the workflow, adds the missing failures and protects real stacks.** It keeps the curl waits as shell and adds `exit 1` after the alert wait. `try-it --auto` exits non-zero on a Loki timeout. The workshop project name and host ports are fixed, so a run on David's Mac could collide with a real stack: the script checks for containers of that project and busy ports first, and refuses to start. Only after the check passes does it arm the exit trap that runs `destroy`, so it removes only what it created. The same rule holds for every real run in every step (section 4).

```
 smoke.sh start
    |-- containers or ports of the project exist? --yes--> exit 1, touch nothing
    '-- no --> arm trap --> setup -> up -> waits -> load-infrahub -> alert wait -> try-it --auto
                                  any failure or end --> destroy (only this run's stack)
```

**D15. Infrahub: plain HTTP, proven against a capture, with a fallback.**

```
 step 10: workshop 'up' gives a live stack (1.1 and 1.11.2)
          capture the real requests
          fallback: uv run infrahubctl (schema) + small Python script (upserts)   <- works first
 step 11: plain-HTTP client built from the capture
          passes on both versions? --yes--> fallback script deleted
                                  '-no--> fallback stays
```

The lead is the plain-HTTP client the answers decided on. The cost of building it from the SDK source alone would be guessing the GraphQL text, so the capture comes first. The shortcut, keeping the SDK and `infrahubctl` for good, adds a Python dependency to a Rust tool and to the smoke run.

**D16. Sonda version range: 1.22 to 1.29.** The workshops pin 1.22.2 and 1.23.1; the generic target is 1.29.0. `nobs sonda` is tested at 1.29.0 and 1.22.2 (S5); the workshop builders and `reset` are tested at 1.22.2 and 1.23.1 through S1. A version in between is not run, and that gap is recorded.

## 6. Departures from what was asked

- Exit codes now follow the child program. The old tools returned 0 (the workshops' tool exits 1 or 2 only in `status`, `alerts` and `setup`). Accepted in the answers file (labcli item 3).
- The program names `labcli` and `netobs` are gone; only `nobs` remains. Accepted (round 1, answer 5).
- `.env` precedence for labcli and netobs users changes to shell, then `.env`, then `.setup.env`. The architect settled it from the code (round 1). Scripts that rely on `.env` beating the shell break.
- `.env` is loaded from the nearest located lab or workshop folder, not from the current folder as labcli and netobs do. Running from a subfolder now reads the parent's `.env`. Commands that need no located folder still read the current folder. Accepted by the architect (round 8, answer 3).
- The Rust smoke run fails when the alert wait or a Loki wait times out. The Python tool does not. Accepted by the architect (round 2, smoke answer 1). `try-it` exits non-zero on a Loki timeout in every mode. Accepted by the architect (round 3).
- Nokia SR Linux is now in scope, and the earlier "SR Linux: out" is replaced. The Arista cEOS labs wait for David's Linux host. Decided by David (round 2).
- Sonda `show` has no config view, because the server returns none, though the Sonda documentation says it does. The plan follows the code.
- `cycle` groups child flow runs when they exist and shows a flat list otherwise. This changes `autocon5 cycle` output. Accepted (round 5, answer 2).
- The first version is not checked in a cloud session. David's goal stays; the check comes with the Linux host. Accepted by the architect (round 4, answer 3).
- `nobs.toml` is not added to the book lab or labcli repositories; labs are found by layout. Accepted (round 4, answer 4).
- The Provider trait appears at step 4, not step 1. Accepted (round 4, answer 5).
- The per-workshop settings hold the 11 values of round 5, answer 1. The hard-coded device names and peers stay in code because they are identical in both workshops. Accepted by the architect (round 5).
- Until step 11, `nobs` runs `infrahubctl` and a small Python script from this repository through `uv run`. The script is deleted when the plain-HTTP client passes on both Infrahub versions. Accepted by the architect (round 5, answer 5; round 8, answer 2). S1 and its `uv`-free script wait for step 11.
- `docker update` and `docker exec` change behaviour from labcli's broken versions, and `docker network ls|prune` no longer needs `--name`. They are bug fixes under the answers' rule 3; the meanings in step 2 are proposed and confirmed in that step.
- `nobs sonda load` refuses a file with no top-level `kind:` key and a JSON file, with a message; Sonda's own catalog skips such files silently.
- `nobs <ws> preflight --skip-c` is accepted as a no-op that prints that layer C is not built, and is excluded from S3. Decided by the architect (round 7).
- Every real run in every step uses only fixtures and names that nobs created, and stops when it finds anything it did not create. Decided by the architect (round 8).
- The screen steps (21 to 23) come after the project file and before the SSH push. The Sonda view could move to just after step 5 if David wants it sooner. No departure is needed for that.
- The workshops' switch to the Rust `nobs` is outside this plan. Until David publishes nobs, the smoke script runs on a copy of the workshops with `nobs.toml` files added. Accepted (round 1, answer 4).
- The labcli example lab is a test fixture only. Accepted (answers file).
- The S9 x86_64 build is untested until David's Linux host exists. The live Infrahub capture and the SSH behaviour are checked only when their steps run.

## 7. Questions for the requester

None open.

HANDOFF: Step 1, the shape: a Cargo workspace with `nobs-core` and `nobs-cli` (clap with derive), the Runner seam with a fake, a concrete compose type with no trait, and `nobs docker ps` with exit-code passthrough, checked by the six project checks and one real run against a compose fixture on the Mac under a unique project name, plus a list of which check tools are installed
