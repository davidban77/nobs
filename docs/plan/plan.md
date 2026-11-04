Draft 2, returned by the second plan session on 2026-11-04 and written here by the architect unchanged. Draft 1 is in git history. The question it ends with is answered in docs/plan/answers-2026-11-04.md; its two investigations run as the discovery stage of the steps that need them (8, 9 and 10).


Every rule file I can see carries version 0.12.0. All ten sections are present, and `.claude/profiles/software.md` is visible. I read only the named sources and changed nothing.

# Plan: nobs, first version

Date: 2026-11-04. Owner of the plan: architect. Status: draft.

## 1. Outcome

When the first version is done, one Rust program, `nobs`, replaces `labcli`, `netobs` and the workshops' Python `nobs`. It runs on David's Mac, on Linux hosts and in cloud sessions. It keeps the command names, flags and exit codes that attendees and scripts type today. It also talks to Sonda over HTTP. A Ratatui screen sits on the same code and shows one lab system at a time, with the Sonda view first.

On the Mac, containerlab runs inside the existing OrbStack Linux machine `clab`, with Nokia SR Linux nodes. The Arista cEOS labs wait for the Linux host that David will supply later.

The workshops switch from the Python `nobs` to the Rust one only after the Rust one passes the smoke run (S1). The switch itself is not part of this plan.

Success checks. A person can watch each one run:

- S1. Smoke, and it can fail. A script in this repository (`scripts/smoke.sh`) runs the workshops' smoke sequence with the Rust `nobs`, once for `autocon5` and once for `packt`: `setup`, `up`, five readiness waits, `load-infrahub`, wait for a firing `BgpSessionNotUp` alert, `try-it --auto`, `destroy`. The steps, waits and timeouts are copied from `reference/workshops/.github/workflows/validate.yml` (see `docs/discovery/workshops-smoke.md`, section 1). Each workshop run stays within 25 minutes. It passes on the Mac (OrbStack's Docker) and on a Linux host (the `clab` machine, or David's host when it exists). The run must exit non-zero when the alert wait or a Loki wait times out. To prove this, stop the `webhook` container after `up` and run the script: it must exit non-zero. The Python tool exits 0 in that case.
- S2. Same exit code. One named command from each old tool runs with the old and the new program on the same fixture, and the exit codes are equal on success.
  - labcli: `docker ps` against the labcli example compose file.
  - netobs: `docker ps -s ch5`.
  - workshops: `nobs autocon5 status`, and `alerts` with a bad label, which exits 2.
  - The old tools run from a copy outside `reference/`, because `uv` writes a virtual environment.
  - The deliberate difference: on failure, `nobs` returns the child program's exit code. The old tools returned 0.
- S3. Surface is kept. A test in this repository lists every command and flag named in the discovery tables (labcli 16, netobs, workshops 3 root plus 12 lifecycle, capability and extra commands). The test fails if one is missing from `nobs --help`.
- S4. containerlab on the Mac. Inside `clab`, the Linux build of `nobs` runs `containerlab deploy`, `inspect` and `destroy` for a two-node SR Linux lab. The real nodes are started, listed and removed.
- S5. Sonda on the Mac. Against a running `sonda-server` 1.29.0 in OrbStack's Docker: load a local runnable file, list shows its id, `stats` shows counters, send one event, delete it, list is empty. A duplicate load prints the server's 409 text. `show` prints what the server returns and no config object.
- S6. Project file. A two-node SR Linux lab is built from the project file inside `clab`. A config is pushed over SSH to one node and read back. The same check on two cEOS nodes waits for the Linux host.
- S7. Screen. `nobs tui` opens the Sonda view and refreshes it every 1 to 2 seconds. Pressing `q` restores the terminal.
- S8. Checks. The six project checks in section 4 pass on the Mac, shown by command output. The Linux build is checked by running it (S4 and S9), because `clab` has no Rust toolchain.
- S9. Install. A release build of `nobs` for Linux arm64 (static) runs `nobs --version` in a plain Linux container with no Rust installed. The Linux x86_64 and macOS builds are built here; the x86_64 one is first run on David's Linux host.

## 2. Scope

In scope:
- The command line of all three tools, with the commands decided in `docs/plan/answers-2026-11-04.md`.
- The Sonda interface over HTTP. It shows only what the server returns, and offers only `kind: runnable` files to post.
- containerlab with Nokia SR Linux nodes on the Mac. The cEOS labs on a Linux host later.
- A project file and build flow for containerlab labs.
- Config push over SSH, last: SR Linux on the Mac if its CLI over SSH is enough, Arista EOS and Cisco IOS on the Linux host.
- A Ratatui screen on the same code, after the command line.
- A place in the design for a GNS3 provider.

Out of scope:
- Anything built for GNS3, labby's Telnet console bootstrap, and the DigitalOcean path.
- Real cEOS runs on the Mac.
- Preflight layer C (Playwright).
- The Prefect secret commands, which stay as Python scripts in the lab.
- A local scenario file check for Sonda, any dependency on `sonda-core`, and Sonda's gNMI and SNMP surface.
- Starting or stopping `sonda-server` from the Sonda interface.
- A library crate for other tools.
- Changing the Python `nobs`, the `reference/` folders, or the workshops repository. This includes adding `nobs.toml` to the real workshops repository. The smoke script runs on a copy of the workshops folders, outside `reference/`, with `nobs.toml` files added.
- Aliases for `labcli` and `netobs`.
- Publishing anything, and a licence file, until David publishes.
- The detailed screen design.

## 3. What this plan rests on

| Statement | Known, inferred or missing | Source | Date checked |
|---|---|---|---|
| The rule files and the software profile are visible, all at version 0.12.0, all ten sections present. | known | `workbench-00-about.md` and the other rule files, `.claude/profiles/software.md` | 2026-11-04 |
| The old tools mostly run external programs (`docker compose`, `docker network`, `containerlab`) and make HTTP calls. They keep no state. | known (code reading) | `docs/discovery/labcli.md`, `lab.md`, `workshops.md` | 2026-11-04 |
| The old tools never pass a failing exit code to the shell. The workshops' tool prints `fail` without exiting, so `try-it --auto` exits 0 even when all four Loki waits time out. | known (code reading) | `docs/discovery/workshops-smoke.md` section 1, `labcli.md`, `lab.md` | 2026-11-04 |
| `.env` precedence is decided: shell, then `.env`, then `.setup.env`. This differs from labcli and netobs, where `.env` beats the shell by accident. | known (code reading, decided) | `docs/plan/answers-2026-11-04.md` round 1 | 2026-11-04 |
| The same command names exist in more than one tool with different flags (`--compose` and `--scenario`; `lab.clab.yaml` and `lab.yml`; network names `labcli` and `network-observability`; `--sudo` defaults). | known | `docs/discovery/labcli.md`, `lab.md` | 2026-11-04 |
| containerlab cannot run natively on macOS. It runs inside a Linux arm64 machine. | known (documentation) | https://containerlab.dev/macos/ (via `docs/discovery/mac.md`) | 2026-11-04 |
| The Mac has an OrbStack Linux machine `clab` (Ubuntu arm64, containerlab 0.77.0, Docker 29.1.3, Compose 2.40.3, the SR Linux image pulled, 15 GiB visible, passwordless sudo, no Rust toolchain). The Mac's files are visible there at the same paths. Its Docker engine is separate from OrbStack's own engine on the Mac side (29.8.2, Compose 5.5.0). | known (checked by the architect on the Mac) | `docs/plan/answers-2026-11-04.md` round 2 | 2026-11-04 |
| SR Linux is native on arm64, so two SR Linux nodes should deploy in `clab`. | inferred (documentation plus one stopped two-node lab that exists) | https://containerlab.dev/macos/, `answers` round 2 | 2026-11-04 |
| arm64 cEOS is not a usable path on the Mac: it is an engineering build behind an Arista login, and the sources disagree about newer builds. cEOS waits for the Linux host. | inferred (the sources conflict; the plan does not depend on either side) | `docs/discovery/mac.md`, `answers` round 2 | 2026-11-04 |
| Memory is enough: 36 GiB on the Mac, no OrbStack limit, 15 GiB inside `clab`. The workshop preflight reads the Mac's total RAM. | known | `answers` round 2, `lifecycle/preflight.py` | 2026-11-04 |
| Ten third-party workshop images publish arm64. The five local builds (`telegraf-srl1`, `telegraf-srl2`, `grafana`, `webhook`, `prefect-flows`), alertmanager, redis, rabbitmq, postgres, python-slim and the Sonda 1.29.0 tag are not checked. | **missing** (the first real `up` in step 6 and the Sonda run in step 5 settle it) | `docs/discovery/mac.md` | 2026-11-04 |
| The smoke job is a fixed sequence of 10 steps with five readiness gates, a 60-try 5-second alert wait, four Loki waits of 60 seconds, a 25-minute limit and a two-workshop matrix. | known (code reading, nothing run) | `docs/discovery/workshops-smoke.md` section 1 | 2026-11-04 |
| The containers need only environment variables passed through compose and the `./automation` mount. No container calls the host tool. A rewritten `INFRAHUB_ADDRESS` must never be exported to the process environment, or compose injects it into `prefect-flows`. | known (code reading) | `docs/discovery/workshops-smoke.md` section 2 | 2026-11-04 |
| The host-side `try-it --auto` needs `.env` merging, the `INFRAHUB_ADDRESS` rewrite and the Infrahub SDK. `load-infrahub` also needs `infrahubctl`. The direct route for the schema is `POST /api/schema/load`. | known | `docs/discovery/workshops-smoke.md` sections 1 and 2 | 2026-11-04 |
| The exact HTTP and GraphQL calls behind the Infrahub SDK (device filter and save, upserts of devices, interfaces and BGP sessions) and behind `infrahubctl schema load`. | **missing** (see INVESTIGATE) | `docs/discovery/workshops-smoke.md` open questions | 2026-11-04 |
| The two workshop plugins differ in settings (name, title, project name, folder, workflow label, cascade prefix, silence hints, evidence query) and in `cycle.py` logic. The 12 commands are the same. | known for six files; inferred for the files compared only by line count | `docs/discovery/workshops-smoke.md` section 3 | 2026-11-04 |
| Packt's `automation/`, `webhook/`, compose file, `.env.example` and catalog were not read. The `packt-cascade-` prefix must match packt's catalog. | **missing** (see INVESTIGATE) | `docs/discovery/workshops-smoke.md` open questions | 2026-11-04 |
| Sonda needs plain HTTP only. `GET /scenarios/{id}` returns `id`, `name`, `state`, `elapsed_secs`, `degraded`, a 9-field `stats` and `pending_ref` (only when `unresolved`), and no config. The fields of `PendingRef` and the keys of the 404 body are unread. | known, with a small **missing** part (step 5 reads `sonda-core` and `not_found`) | `docs/discovery/workshops-smoke.md` section 5, `reference/sonda/sonda-server/src/routes/scenarios.rs` | 2026-11-04 |
| The generic Sonda default is `http://localhost:8080`. The workshops use 8085 through `SONDA_SERVER_URL`. | known (decided) | `answers` (Sonda 4) | 2026-11-04 |
| Labby's shape is clear: Provider, Project, Node and Link classes, a project file with `main`, `nodes_spec`, `links_spec` and `vars`, and the flow topology, bootstrap, configs. Its SSH path hardcodes credentials, which must not be copied. | known (code reading) | `docs/discovery/labby.md` | 2026-11-04 |
| How a containerlab topology and SR Linux startup configs map onto labby's build flow. | **missing** (step 13 settles it) | containerlab documentation; `reference/labcli/example/containerlab/lab.clab.yml` is a lead | 2026-11-04 |
| The crates are chosen: clap (derive, env), serde-saphyr, toml, minijinja, ureq 3, russh with tokio inside the SSH module, ratatui 0.30 with crossterm 0.29. Versions as of 2026-11-04. crossterm has had no release for 19 months. | known (crates.io), decided | `docs/discovery/crates.md`, `answers` round 2 | 2026-11-04 |
| clap nested groups, aliases and `env` flags work as the plan needs. | inferred (the report took this from background knowledge, not a read source) | `docs/discovery/crates.md` | 2026-11-04 |
| russh may pull C crypto code that a static Linux cross build needs a C cross compiler for. | inferred risk | `docs/discovery/crates.md` | 2026-11-04 |
| No mature Rust crate gives scrapli-style prompt handling. `rneter` is the closest and is young. | known (crates.io); fitness inferred | `docs/discovery/crates.md` | 2026-11-04 |
| Whether SR Linux's CLI over SSH is enough for config push. | **missing** (step 16 settles it) | none yet | 2026-11-04 |

## 4. Work breakdown

Sizes are guesses for one factory run: S is up to about 500 lines with tests, M up to about 1500, L more.

Project checks. They are the same for every step, and every step runs them on the Mac. No task runner file exists yet.

1. `cargo build --workspace`
2. `cargo nextest run --workspace`
3. `cargo test --workspace --doc`
4. `cargo clippy --workspace --all-targets -- -D warnings`
5. `cargo fmt --all -- --check`
6. `cargo audit`

Real runs use the Mac's OrbStack Docker for Sonda and the workshop stacks, and `orb -m clab <command>` for everything that needs containerlab. Nothing runs inside `reference/`; copies go to a git-ignored scratch folder.

| Step | Content | Proves |
|---|---|---|
| 1 (S) | **Shape.** A Cargo workspace with `nobs-core` (library, `publish = false`) and `nobs-cli` (the `nobs` binary, clap with derive). The core holds the Runner, which runs an external program from a list of arguments and returns its exit status. It also holds a small Provider trait with one method, `show`, and the compose provider. The binary has one command, `nobs docker ps` with `--compose` (default `./docker-compose.yml`). It builds `docker compose -f FILE ps --all`, as labcli does. A missing file exits 1. The child's exit code is passed back. Test fixture: a one-service compose file. No `.env` loading, no HTTP, no other dependency. | The layout builds and passes the six checks. The command line equals labcli's (a test on the argument list). A fake Runner returning 3 makes `nobs` exit 3, and the test fails if passthrough is removed. On the Mac, the fixture is started with `docker compose` and `nobs docker ps` shows it (real run). |
| 2 (M) | **Compose commands.** All labcli `docker` commands: build, pull, exec, debug, start, stop, restart, logs, ps, destroy, rm, update and network. `update` and `exec` are rebuilt properly. `.env` and `.setup.env` load with the order shell, `.env`, `.setup.env`. Union flags `--compose` and `--scenario`. The netobs `docker` names are kept. `DOCKER_COMPOSE_WITH_HASH` is dropped. The labcli example is the test fixture. | Argument lists equal the labcli table, one test per command. Precedence tests fail if the order changes (shell beats `.env`, `.env` beats `.setup.env`). A real start, ps, logs, exec and destroy on the Mac. |
| 3 (S) | **Linux arm64 build inside `clab`.** A static `aarch64-unknown-linux-musl` release build made on the Mac, run from its build folder inside `clab`. No installer: the file is visible at the same path. The build route is part of the step. If the Mac lacks the target or a linker, say what was added. A fallback is a native arm64 Linux build in a container on OrbStack. | `orb -m clab <path>/nobs --version` prints the version. `nobs docker ps` against the step 1 fixture lists a service started with `clab`'s own Docker. |
| 4 (M) | **containerlab provider, SR Linux first.** `containerlab deploy`, `destroy`, `inspect`, with `--sudo`, the exact command names, and both default topology names. A two-node SR Linux fixture topology owned by this repository (image `ghcr.io/nokia/srlinux:latest`). The second Provider implementation. A real `sudo` run keeps the environment variables (labcli conclusion 7). | The trait fits two providers, or changes shape here. Argument lists with a fake Runner. S4: the Linux build deploys, inspects and destroys the real two-node lab in `clab`. The test fails if `--sudo` is dropped. |
| 5 (M) | **Sonda interface, command line.** The `nobs sonda` group: health, ready, list, show, stats, delete, load (a local file), event, metrics, version from `sonda_server_build_info`. Flag first, then `SONDA_SERVER_URL` and `SONDA_API_KEY`. Default `http://localhost:8080`. The key is never written to a file. `show` prints the fields the server returns and no config. The step reads `PendingRef` and the 404 body in `reference/sonda`. The build-info line is read with a few lines of own parsing, not a new crate. Scenario files go to the server unparsed. | Tests against a stub HTTP server cover each endpoint, auth, and the 400, 401, 404, 409, 422, 429 and 502 text. S5 on the Mac, including a check that a 1.29.0 arm64 image exists. |
| 6 (M) | **Locator and workshop lifecycle.** The `nobs.toml` locator. Root commands `setup` (preflight and bootstraps, no `uv sync`), `preflight` (layers on the host), `workshops`. Lifecycle commands: setup, up, down, destroy, restart, ps, logs, exec, build. Bare `nobs <command>` inside a workshop folder. The foreign-container guard. `up` has no `--build`, as today. The first real `up` of a copied `autocon5` stack on the Mac, with its five local builds, in OrbStack's Docker. | A workshop is found from any folder below it. `up` refuses when the other workshop's containers exist (the guard test fails if removed). The real `up` shows whether the local builds work on arm64; if one fails, the step reports which and stops. |
| 7 (L) | **Workshop service checks.** The HTTP clients for Prometheus, Loki, Alertmanager, Grafana, Prefect and the webhook. Commands: `status`, `alerts` (exit 1 and 2 as today), `silences`. | `status` exits 1 when a service is down and 0 when all are up, against stub servers and then the real stack from step 6. `alerts --label bad` exits 2. |
| 8 (M) | **Infrahub.** `load-infrahub`, `schema load`, `maintenance`. Plain HTTP and GraphQL with the `X-INFRAHUB-KEY` header. No `infrahubctl` and no SDK. The `INFRAHUB_ADDRESS` rewrite from `infrahub-server` to the host form is made per call and never exported. The probe-then-load step used by `reset`. Starts from the INVESTIGATE answer on the SDK's calls. | The schema loads and the devices upsert twice in a row without error (repeatable). The stub test fails if the key header is dropped. A test fails if the rewrite is exported to the process environment. |
| 9 (M) | **Flow commands.** `cycle` (packt's grouped view for both workshops), `evidence`, `rca`, `try-it` (including `--auto`), `page`. Per-workshop settings (title, project name, folder, workflow label, cascade prefix, silence hints, evidence log query, image pins) come from `nobs.toml`. `try-it` exits non-zero when a Loki wait times out (see Q1). | `try-it --auto` runs the same four paths as the smoke run against stubs, and exits non-zero when a wait times out (the test fails if that is removed). The two evidence queries differ (two tests). `cycle` groups subflow rows under their parent for both workshops. |
| 10 (L) | **Sonda scenario builders and the smoke script.** `flap-interface`, `incident`, `scenarios`, `reset`, and per-workshop `preflight` layers A and B. The builders build the v2 scenario body in memory and POST it. Then `scripts/smoke.sh`, with the steps, waits and timeouts of the workflow. It exits non-zero when the alert wait times out, and it dumps container logs on failure. It destroys the stack on every exit. It runs the two workshops one after the other. | The cascade bodies equal the Python bodies for the same input (pytest cases as reference). **Milestone M1: S1 passes on the Mac, and fails with the webhook stopped.** The same run on a Linux host (`clab`) follows. |
| 11 (L) | **Book lab.** `lab deploy`, `destroy`, `purge`, `show`, `prepare`, `update`, `rebuild`. Scenarios are listed from `chapters/`. The network is created first. The program stops on a failed step and names it. It waits for services. It checks for the cEOS image and says how to import it. The flap helper reads its credentials from the environment. Inside `clab` one Docker engine serves containerlab and compose, so the shared network works. | A step failure stops the run and names the step (fake Runner). In `clab`: the compose parts of a chapter against the shared network, with the SR Linux fixture. The real cEOS `lab deploy` waits for the Linux host. |
| 12 (M) | **Nautobot helpers.** `utils load-nautobot` (device type bug fixed, repeatable), `delete-nautobot`. | A second load changes nothing. A cEOS device is not created with the SR Linux type (the test fails on the old bug). |
| 13 (L) | **Project file and build flow.** `nobs_project.yml` (labby's shape: main, nodes, links, vars), read with serde-saphyr, for containerlab. Templates with minijinja. The flow renders the topology and startup configs, then deploys. | A two-node SR Linux project renders the expected topology file (golden). In `clab`: it deploys. The YAML crate keeps `lab_vars.yml` values unchanged (a round-trip test). |
| 14 (M) | **Screen base and Sonda view.** `nobs-tui` crate (Ratatui 0.30, crossterm). It uses only `nobs-core` calls. A list of scenarios refreshed every 1 to 2 seconds, with the selected scenario's stats and the fields `show` returns. Only runnable files can be posted. Packs are read-only. | S7. A test of the data layer without a terminal. A person watches it open and quit. |
| 15 (L) | **Screens per lab system.** Compose stack, containerlab lab, book lab and the workshop view, one lab system at a time. The containerlab view runs from the Linux build inside `clab`. | A person drives each one; the CLI behaviour is unchanged (S2 and S3 still pass). |
| 16 (L) | **Config push over SSH.** Per-node config from a template plus variables. First task: a hands-on check of `rneter` against a real SR Linux node. Fallback: own prompt handling on russh. Tokio stays inside the SSH module. Credentials come from the project file or the environment, never hardcoded. Arista EOS and Cisco IOS wait for the Linux host. | S6 on the Mac (SR Linux). A wrong password is reported, not hidden. S9 is repeated, because russh may change the static build. |
| 17 (S) | **Packaging check.** Release builds for macOS arm64 and Linux (x86_64 and aarch64, static), the Linux arm64 one checked in a plain Linux container. | S9. |

The first change (step 1) is deliberately small. It proves the crate layout, the Runner seam, one provider, one real command with tests, and the six checks. It does not touch `.env`, HTTP, the screen or the Linux build.

## 5. Decisions

All decisions are **proposed** until the requester agrees. Decisions D1 to D12 keep the draft 1 reasoning, with the changes marked.

**D1. Crate layout: a workspace with `nobs-core` and `nobs-cli`; `nobs-tui` is added at step 14.**
- The lead option is a workspace of three crates, where the core never prints or reads a terminal. The screen then reuses the core with no copying.
- The shortcut is one crate with `lib` and `main`. It saves about 30 lines of manifest, but nothing stops terminal code from leaking into the core.
- Two programs (`nobs` and `nobs-tui`) break "Rust, one program". Rejected.

```
   nobs-cli  (binary `nobs`)         nobs-tui  (Ratatui, step 14)
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

**D2. Command line library: `clap` with derive and env.** A usage error exits 2, as Typer does today. Risk: workshop names come from files at run time, so a fallback looks up an unknown first word as a workshop and parses the rest with the workshop command set. Step 6 proves it.

**D3. Running external programs: argument lists through a `Runner` trait, no shell.**

```
 command --> build the argument list (pure function) --> Runner
                                                          |-- real: std::process::Command, terminal inherited
                                                          '-- fake: records the list, returns a chosen exit code
```

Tests check the argument list and the exit code with no Docker running. A Docker API crate covers nothing we need, because compose and containerlab have no API.

**D4. Formats and crates.**
- TOML (`toml`) for nobs's own settings and the locator. YAML (`serde-saphyr`, with the `%YAML 1.2` output line turned off) for the project file and `lab_vars.yml`.
- nobs parses only its own files. Scenario files go to Sonda unparsed, so Sonda stays the judge of what a valid scenario is.
- Templates: `minijinja`, pinned to an exact version because it is on a fresh major version.
- Cost: two parsers, and Apache-2.0-only crates (minijinja, russh), which the answers accept.

**D5. Screen's place: a separate crate over the core, started with `nobs tui`.** Plain `nobs` prints help. No cargo feature switch for now. The core exposes data and polling calls, not printed text. Crossterm has had no release for 19 months; this is a risk to watch.

**D6. The core is synchronous.**
- HTTP is `ureq` 3. Threads and channels carry polling to the screen.
- Tokio is used only inside the SSH module (russh).
- Every new dependency is named in its change description.

**D7. Provider trait.** The trait starts with one method, `show`. Both compose `ps` and containerlab `inspect` fit it. Containerlab (step 4) is the second implementation and the real test, now earlier than in draft 1.
- The shortcut is a concrete compose type now and the trait at step 4. It is cheaper, but the provider idea stays unproven.
- A compose stack is services; a lab is nodes and links. Do not force both into one noun.
- The reviewer must refuse extra methods in step 1.

**D8. One binary, `nobs`, and no aliases.** Groups `docker`, `containerlab`, `lab`, `utils`, `setup`, `sonda`, `tui`, and `<workshop> <command>`. The old command names stay. Flags are the union. The network name default comes from the layout. Exit codes are passed through; a usage error is 2. The README maps `labcli X` and `netobs X` to `nobs X`.

**D9. Locator file `nobs.toml`.** It sits in a workshop or lab folder. The nearest file above the current folder wins. `--dir` and `NOBS_DIR` override. It also carries the per-workshop settings.

```
Before:  Python code in each plugin; _repo_root() guesses four folders up
After:   workshops/packt/nobs.toml
           kind = "workshop"
           name = "packt"
           compose_file = "docker-compose.yml"
           workflow_label = "packt_quarantine_bgp"
           cascade_prefix = "packt-cascade-"
           evidence_query = "<packt's query>"
```

The alternative is a global list in `~/.config/nobs`. It breaks in cloud sessions that start empty.

**D10. Sonda defaults differ on purpose.** `nobs sonda` uses 8080. Workshop commands keep 8085 from their `.env`.

**D11. Test strategy.** A fake Runner for argument lists, a stub HTTP server for the clients, a surface test (S3), and real runs on the Mac. A test must fail when the behaviour it names is removed. Old pytest cases are references for the cascade builders. The old tools and the workshops run from copies outside `reference/`.

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

- Option A is the lead. The program inside `clab` is the same program that runs on a Linux host, with no Mac-specific code. Everything it does there is what David's Linux host will do later. Cost: a second build (step 3, S) and a longer command; a shell alias covers that. Risk: a static cross build may need a linker or a container build, and russh (step 16) may complicate it.
- Option B is the shortcut. One Mac binary is easier to type. But it ties nobs to OrbStack, must translate paths and carry `sudo` and environment variables across `orb`, and the Mac behaves differently from Linux. It can come later as a small Runner variant, so A does not close it.
- Open point not to forget: the two Docker engines on the Mac do not share networks. A person must know which build talks to which engine. The README says so.

**D14. The smoke script copies the workflow and adds the missing failures.** It keeps the curl waits as shell and adds `exit 1` after the alert wait. `try-it --auto` exits non-zero on a Loki timeout. A smoke run that cannot fail is not a check. The script needs no `uv`, because `setup` only copies `.env.example` and the schema loads over HTTP.

## 6. Departures from what was asked

- Exit codes now follow the child program. The old tools returned 0. Accepted in the answers file (labcli item 3).
- The program names `labcli` and `netobs` are gone; only `nobs` remains. Accepted (round 1, answer 5).
- `.env` precedence for labcli and netobs users changes to shell, then `.env`, then `.setup.env`. The architect settled it from the code (round 1). Scripts that rely on `.env` beating the shell break.
- The Rust smoke run fails when the alert wait or a Loki wait times out. The Python tool does not. Accepted by the architect (round 2, smoke answer 1). The exit code of `try-it` itself on a timeout is Q1.
- Nokia SR Linux is now in scope, and the earlier "SR Linux: out" is replaced. The Arista cEOS labs wait for David's Linux host. Decided by David (round 2).
- Sonda `show` has no config view, because the server returns none, though the Sonda documentation says it does. The plan follows the code.
- `cycle` uses packt's grouped view for both workshops. This changes `autocon5 cycle` output. Accepted (round 2, smoke answer 3).
- The screen steps (14 and 15) come after the project file and before the SSH push, which is the last functional step. The Sonda view could move to just after step 5 if David wants it sooner. No departure is needed for that.
- The workshops' switch to the Rust `nobs` is outside this plan. Until David publishes nobs, the smoke script runs on a copy of the workshops with `nobs.toml` files added. Accepted (round 1, answer 4).
- The labcli example lab is a test fixture only. Accepted (answers file).

## 7. Questions for the requester

- Q1. When a Loki wait times out, should `try-it` exit non-zero in every mode, or only with `--auto`? The accepted answer says the smoke run fails on a timeout. It does not say whether the interactive command changes too. Reading one: every mode, so the command has one meaning and the smoke script needs no flag. Reading two: only `--auto`, so attendees who run `try-it` by hand keep today's exit code 0 on a timeout. Default: every mode, listed as a departure.

## Status lines

DECISION NEEDED: When a Loki wait times out, should `try-it` exit non-zero in every mode, or only with `--auto`? Default: every mode, listed as a departure.
INVESTIGATE: What exact HTTP and GraphQL calls does the Infrahub SDK make for the device filter and save, and the upserts of devices, interfaces and BGP sessions, and what does `infrahubctl schema load` post, so that nobs can call Infrahub directly? Sources: https://docs.infrahub.app, https://github.com/opsmill/infrahub-sdk-python, `reference/workshops/workshops/autocon5/src/autocon5_workshop/load.py`, `reference/workshops/workshops/autocon5/src/autocon5_workshop/try_it.py`, `reference/workshops/.github/workflows/validate.yml`
INVESTIGATE: What do packt's `automation/`, `webhook/`, compose file, `.env.example` and Sonda catalog contain, and do they match the autocon5 differences already found (including the `packt-cascade-` prefix and the files compared only by line count)? Sources: `reference/workshops/workshops/packt`, `reference/workshops/workshops/autocon5/src`
HANDOFF: Step 1, the shape: a Cargo workspace with `nobs-core` and `nobs-cli` (clap with derive), the Runner seam with a fake, a one-method Provider trait with the compose provider, and `nobs docker ps` with exit-code passthrough, checked by the six project checks and one real run against a compose fixture on the Mac
