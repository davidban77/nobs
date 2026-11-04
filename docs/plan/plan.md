Draft 1, returned by the plan session on 2026-11-04 and written here by the architect unchanged. The questions it ends with are answered in docs/plan/answers-2026-11-04.md.


# Plan: nobs, first version

Date: 2026-11-04. Owner of the plan: architect. Status: draft.

## 1. Outcome

When the first version is done, one Rust program, `nobs`, replaces `labcli`, `netobs` and the workshops' Python `nobs`. It runs on David's Mac and on Linux hosts and cloud sessions. It keeps the command names, flags and exit codes that attendees and scripts type today.

It also talks to Sonda over HTTP. A Ratatui screen sits on the same code and shows one lab system at a time, with the Sonda view first.

The workshops switch to the Rust `nobs` only after it passes their scheduled smoke run. That switch is not part of this plan (see Q4).

Success checks. A person can watch each one run:

- S1. Smoke. On a Linux host, the Rust `nobs` runs the workshops' smoke sequence for both `autocon5` and `packt`: `setup`, `up`, wait until ready, `load-infrahub`, wait for a firing `BgpSessionNotUp`, `try-it --auto`, `destroy`. Every step exits 0. The exact steps come from `reference/workshops/.github/workflows/validate.yml`.
- S2. Same exit code. One named command from each old tool runs with the old and the new program on the same fixture. On success the exit codes are equal.
  - labcli: `docker ps` against the labcli example compose file.
  - netobs: `docker ps -s ch5`.
  - workshops: `nobs autocon5 status`, and `alerts` with a bad label, which exits 2.
  - The deliberate difference: on failure, `nobs` returns the child program's exit code. The old tools returned 0 (answers file, labcli item 3).
- S3. Surface is kept. A test in this repository lists every command and flag named in the discovery tables (labcli 16, netobs, workshops 3 root plus 12 lifecycle, capability and extra commands). The test fails if one is missing from `nobs --help`.
- S4. containerlab. On a Linux host, `nobs containerlab deploy`, `inspect` and `destroy` bring up and remove the book lab's two cEOS nodes. On the Mac, only the command lines are checked, with a fake program (see section 3).
- S5. Sonda. Against a running `sonda-server` 1.29.0: load a local file, list shows its id, `stats` shows counters, send one event, delete it, list is empty. A duplicate load prints the server's 409 text.
- S6. Project file. A two-node containerlab lab is built from the project file on a Linux host. A config is then pushed over SSH to a cEOS node and read back.
- S7. Screen. `nobs tui` opens the Sonda view and refreshes it every 1 to 2 seconds. Pressing `q` restores the terminal.
- S8. Checks. The project checks in section 4 pass on the Mac and on a Linux host, shown by command output.
- S9. Install. A release build of `nobs` for Linux runs `nobs --version` inside a plain Linux container with no Rust installed.

## 2. Scope

In scope:
- The command line of all three tools, with the commands decided in `docs/plan/answers-2026-11-04.md`.
- The Sonda interface over HTTP.
- A project file and build flow for containerlab labs, and config push over SSH to Arista EOS and Cisco IOS.
- A Ratatui screen built on the same code, after the command line.
- A place in the design for a GNS3 provider.

Out of scope:
- Anything built for GNS3, labby's Telnet console bootstrap, and the DigitalOcean path.
- Nokia SR Linux.
- Preflight layer C (Playwright).
- The Prefect secret commands, which stay as Python scripts in the lab.
- A local scenario file check for Sonda, and Sonda's gNMI and SNMP surface.
- Starting or stopping `sonda-server` from the Sonda interface.
- A library crate for other tools to import.
- Changing the Python `nobs`, the `reference/` folders, or the workshops repository.
- Publishing anything.
- The detailed screen design.

## 3. What this plan rests on

| Statement | Known, inferred or missing | Source | Date checked |
|---|---|---|---|
| Rule files and the software profile are visible, all at version 0.12.0, all ten sections present. | known | `workbench-00-about.md` and the other rule files, `.claude/profiles/software.md` | 2026-11-04 |
| The old tools mostly run external programs (`docker compose`, `docker network`, `containerlab`) and make HTTP calls. They keep no state of their own. | known (code reading, nothing run) | `docs/discovery/labcli.md`, `lab.md`, `workshops.md` | 2026-11-04 |
| The old tools never pass a failing exit code to the shell. The workshops' tool does return 1 and 2 in places. | known (code reading) | `docs/discovery/labcli.md`, `lab.md`, `workshops.md` | 2026-11-04 |
| The effective precedence between the shell environment, `.env` and `.setup.env` in labcli and netobs is unclear. The code loads `.env` with override into the process environment, then says the process environment wins. That may mean `.env` beats the shell. The workshops' tool states that the process environment wins, and tests pin it. | **missing** (never run) | `reference/labcli/labcli/base.py:13-14`, `reference/network-observability-lab/netobs/main.py:25-26`, `reference/workshops/packages/nobs/src/nobs/lifecycle/env.py` | 2026-11-04 |
| The same command names exist in more than one tool with different flags: `docker start` takes `--compose` in labcli and `--scenario` in netobs. `containerlab` commands default to `lab.clab.yaml` in labcli and `lab.yml` in netobs. The network name defaults differ (`labcli` and `network-observability`), and so do the `--sudo` defaults (the `lab` commands default to false). | known | `docs/discovery/labcli.md`, `lab.md` | 2026-11-04 |
| containerlab is not installed on the Mac. How to run it there (inside a Linux container under OrbStack, or not at all) is unknown. | **missing** | Task limits | 2026-11-04 |
| Whether the workshop stack (22 services) and the Sonda image run on arm64 under OrbStack is unknown. | **missing** | `docs/discovery/workshops.md`, `sonda.md` | 2026-11-04 |
| The Prefect flows, the webhook and the AI step run in containers. The host-side tool only starts, looks at and triggers them. What the flows need from the host is not read. | inferred, with a **missing** part | `docs/discovery/workshops.md` (`flows.py`, `workshop_sdk.py` not read) | 2026-11-04 |
| The smoke run is the main behaviour reference. Only 9 pytest files exist, and they cover lifecycle, env, registry and the cascade builders. | known | `docs/discovery/workshops.md` | 2026-11-04 |
| The smoke run has other steps (waits, timeouts, how it installs `nobs`) that only a summary describes. | **missing** | `reference/workshops/.github/workflows/validate.yml` | 2026-11-04 |
| autocon5 and packt share the same 12 extra commands. Few differences were found, but the files were not diffed. | inferred | `docs/discovery/workshops.md` | 2026-11-04 |
| Sonda needs plain HTTP only. The API has no pause, resume or streaming. The shape of `GET /scenarios/{id}` is not documented. Nobody has run Sonda for this plan. | known from docs, shape **missing** | `docs/discovery/sonda.md` | 2026-11-04 |
| The generic Sonda default is `http://localhost:8080` (decided). The workshops use 8085 through `SONDA_SERVER_URL` in their `.env`. | known | `docs/plan/answers-2026-11-04.md` (Sonda 4), `docs/discovery/sonda.md` | 2026-11-04 |
| Labby's shape is clear: abstract Provider, Project, Node and Link classes with one working implementation. Its project file sections are `main`, `nodes_spec`, `links_spec` and `vars`. The build flow is topology, then bootstrap, then configs. Its SSH path hardcodes credentials, which must not be copied. | known (code reading) | `docs/discovery/labby.md` | 2026-11-04 |
| How containerlab topology and cEOS startup configs map onto labby's build flow is unknown. | **missing** | containerlab documentation; `reference/labcli/example/containerlab/lab.clab.yml` is a lead | 2026-11-04 |
| Maintained Rust crates for YAML, Jinja-style templates, HTTP and SSH are not checked. Candidates: `clap`, `serde`, `toml`, `ratatui`, `crossterm`, `minijinja`, `ureq` or `reqwest`, `russh`. The YAML crate `serde_yaml` is archived, to my knowledge, so a replacement must be chosen. | **missing**; my knowledge, not from a source | none; discovery must check crates.io | 2026-11-04 |
| Writing prompt handling for SSH config push in Rust is real work. No crate matching scrapli or netmiko is known. | inferred risk | `docs/discovery/labby.md` | 2026-11-04 |

## 4. Work breakdown

Sizes are guesses for one factory run: S is up to about 500 lines with tests, M up to about 1500, L more.

Project checks. They are the same for every step, and every step runs them on the Mac. No task runner file exists yet.

1. `cargo build --workspace`
2. `cargo nextest run --workspace`
3. `cargo test --workspace --doc`
4. `cargo clippy --workspace --all-targets -- -D warnings`
5. `cargo fmt --all -- --check`
6. `cargo audit`

| Step | Content | Proves |
|---|---|---|
| 1 (S) | **Shape.** A Cargo workspace with `nobs-core` (library) and `nobs-cli` (the `nobs` binary). The core holds the Runner, which runs an external program from a list of arguments and returns its exit status. It also holds a small Provider trait with one method, `show`, and the compose provider. The binary has one command, `nobs docker ps` with `--compose` (default `./docker-compose.yml`). It builds `docker compose -f FILE ps --all`, as labcli does. A missing file exits 1. The child's exit code is passed back. Test fixture: a one-service compose file. No `.env` loading and no HTTP yet. | The layout builds and passes the six checks. The command line equals labcli's (a test on the argument list). A fake Runner returning 3 makes `nobs` exit 3, and the test fails if passthrough is removed. On the Mac, the fixture is started with `docker compose` and `nobs docker ps` shows it (real run). |
| 2 (M) | **Compose commands.** All labcli `docker` commands: build, pull, exec, debug, start, stop, restart, logs, ps, destroy, rm, update and network. `update` and `exec` are rebuilt properly. `.env` and `.setup.env` are loaded with the precedence the investigation settles. Union flags: `--compose` and `--scenario`. The netobs `docker` names are kept. The labcli example is the test fixture. | Argument lists equal the labcli table, one test per command. Precedence tests fail if the order changes. A real start, ps, logs, exec and destroy on the Mac against the fixture. |
| 3 (M) | **Sonda interface, command line.** The `nobs sonda` group: health, ready, list, show, stats, delete, load (a local file), event, metrics, version from `sonda_server_build_info`. Flag first, then `SONDA_SERVER_URL` and `SONDA_API_KEY`. Default `http://localhost:8080`. The key is never written to a file. The HTTP crate is chosen here, after the health check. | Tests against a stub HTTP server cover each endpoint, auth, and the 400, 401, 404, 409, 422, 429 and 502 text. A real run against Sonda on the Mac, if an arm64 image exists (S5). |
| 4 (M) | **Locator and workshop lifecycle.** The `nobs.toml` locator. Root commands `setup`, `preflight` (layers on the host), `workshops`. Lifecycle commands: up, down, destroy, restart, ps, logs, exec, build, setup. Bare `nobs <command>` inside a workshop folder. The foreign-container guard. | A workshop is found from any folder below it. `up` refuses when the other workshop's containers exist (the guard test fails if removed). Real run on the Mac with a small compose fixture. |
| 5 (L) | **Workshop service checks.** The HTTP clients for Prometheus, Loki, Alertmanager, Grafana, Prefect and the webhook. Commands: `status`, `alerts` (exit 1 and 2 as today), `silences`. | `status` exit 1 when a service is down and 0 when all are up, against stub servers. `alerts --label bad` exits 2. |
| 6 (M) | **Infrahub.** `load-infrahub`, `schema load`, `maintenance`. Plain HTTP and GraphQL, with the `X-INFRAHUB-KEY` header. The probe-then-load step used by `reset`. | The schema loads and the devices upsert twice in a row without error (repeatable). The stub test fails if the key header is dropped. |
| 7 (M) | **Flow commands.** `cycle`, `evidence`, `rca`, `try-it` (including `--auto`), `page`. Per-workshop settings (workflow label, evidence log query, image pins) come from `nobs.toml`. | `try-it --auto` runs the same steps as the smoke run against stubs. The packt log query differs from the autocon5 one (two tests). |
| 8 (L) | **Sonda scenario builders.** `flap-interface`, `incident`, `scenarios`, `reset`, and per-workshop `preflight` layers A and B. The builders build the v2 scenario body in memory and POST it (decided: they belong to the workshop change). | The cascade bodies equal the Python bodies for the same input (pytest cases as reference). **Milestone M1: S1, the smoke sequence, passes on a Linux host.** |
| 9 (M) | **containerlab provider.** `containerlab deploy`, `destroy`, `inspect`, with `--sudo`, the exact command names, and both default topology names. The second Provider implementation. | The trait fits two providers, or changes shape here. On the Mac: argument lists with a fake `containerlab`. On Linux: S4. |
| 10 (L) | **Book lab.** `lab deploy`, `destroy`, `purge`, `show`, `prepare`, `update`, `rebuild`. Scenarios are listed from `chapters/`. The network is created first. The program stops on a failed step and says which step failed. It waits for services. It checks for the cEOS image and says how to import it. The flap helper reads its credentials from the environment. | A step failure stops the run and names the step (fake Runner). On the Mac: `docker`-only parts with a fixture. On Linux: a real `lab deploy`. |
| 11 (M) | **Nautobot helpers.** `utils load-nautobot` (device type bug fixed, repeatable), `delete-nautobot`. | A second load changes nothing. A cEOS device is not created with the SR Linux type (test fails on the old bug). |
| 12 (L) | **Project file and build flow.** `nobs_project.yml` (labby's shape: main, nodes, links, vars) for containerlab. The flow renders the topology and startup configs, then deploys. | A two-node project renders the expected topology file (golden). On Linux: it deploys. |
| 13 (L) | **Config push over SSH.** A per-node config from a template plus variables, to Arista EOS and Cisco IOS first. The credentials come from the project file or environment, never hardcoded. Starts with a short check of the SSH crate (see INVESTIGATE). | S6 on Linux. A wrong password is reported, not hidden. |
| 14 (M) | **Screen base and Sonda view.** `nobs-tui` crate (Ratatui). It uses only `nobs-core` calls. A list of scenarios refreshed every 1 to 2 seconds, with the selected scenario's stats. | S7. A test of the data layer without a terminal. A person watches it open and quit. |
| 15 (L) | **Screens per lab system.** Compose stack, containerlab lab, book lab, and the workshop view, one lab system at a time. | A person drives each one; the CLI behaviour is unchanged (S2, S3 still pass). |
| 16 (S) | **Packaging check.** Release builds for macOS arm64 and Linux (x86_64 and aarch64, static), checked in a plain Linux container. | S9. |

The first change (step 1) is deliberately small. It proves the crate layout, the Runner seam, one provider, one real command with tests, and the six checks. It does not touch `.env`, HTTP or the screen.

## 5. Decisions

All decisions are **proposed** until the requester agrees.

**D1. Crate layout: a workspace with `nobs-core` and `nobs-cli`; `nobs-tui` is added at step 14.**

Options compared:
- (a) A workspace of three crates, where the core never prints or reads a terminal. This is the lead option. The screen needs the same operations as the command line. If the core returns data and the command line prints it, the screen reuses it with no copying.
- (b) One crate with `lib` and `main`. It is cheaper by about 30 lines of manifest and one `-p` flag. But nothing stops terminal code from leaking into the core, which is the usual way a screen ends up with its own copy of the logic.
- (c) Several programs (`nobs` and `nobs-tui`). It breaks "Rust, one program". Rejected.

```
   nobs-cli  (binary `nobs`)         nobs-tui  (Ratatui, step 14)
   clap commands, printing,          screens, keys, refresh
   exit codes                                 
            \                          /
             v                        v
              nobs-core   (no printing, no terminal)
       providers: compose | containerlab | (gns3: slot kept)
       runner:    runs external programs, returns the exit status
       clients:   sonda, prometheus, loki, alertmanager, infrahub, ...
       settings:  .env, nobs.toml, nobs_project.yml
```

`nobs-core` is an internal crate (`publish = false`). It is not the "library for other tools" that the answers file rules out (see Q7).

**D2. Command line library: `clap` with derive.**
- It gives nested groups, aliases, `env =` on flags, repeated flags, shell completion and help. A usage error exits 2, as Typer does today.
- Alternatives are `argh`, `lexopt` and `bpaf`. They are smaller but lack aliases or completion, and we would rebuild them.
- Risk: workshop names come from files at run time. Plan: a fallback for unknown first words that looks up a workshop and parses the rest with the workshop command set. Step 4 proves it.

**D3. Running external programs: argument lists through a `Runner` trait, no shell.**

```
 command --> build the argument list (pure function) --> Runner
                                                          |-- real: std::process::Command, terminal inherited
                                                          '-- fake: records the list, returns a chosen exit code
```
- The old tools split strings with `shlex`. That invites quoting bugs (labby builds a shell string from a user name).
- Tests check the argument list and the exit code with no Docker running.
- Alternative: a Docker API crate. Compose and containerlab have no API, so it covers nothing we need.
- A real `sudo` run must keep the environment variables (labcli conclusion 7). Step 9 decides how.

**D4. Formats: TOML for nobs's own settings and the locator; YAML for the project file.**
- Lead option. Rust is at home with TOML, and it allows comments. The project file is YAML because labby's `labby_project.yml`, containerlab topologies, compose files and the lab's `lab_vars.yml` are all YAML. Users already write it, and it stays close to labby so a GNS3 provider can follow.
- Alternatives are all YAML (one parser, but the settings file gets YAML's pitfalls) and all TOML (one parser, but it breaks the labby shape).
- Cost: two parsers. The YAML crate must be checked for upkeep first.
- Templates: Jinja2 is labby's choice, so `minijinja` is the candidate. Check it at step 12.

**D5. Screen's place: a separate crate over the core, started with `nobs tui`.**
- No cargo feature switch for now: the feature would add `cfg` work for little gain. Revisit if a CLI-only build matters. Plain `nobs` prints help, as today, so scripts are unaffected (Q6).
- The core exposes data and polling calls, not printed text.

**D6. Core is synchronous.** Threads and channels carry polling to the screen. That is enough for a 1 to 2 second refresh. The HTTP crate is chosen at step 3, and `ureq` is the lead candidate. The SSH crate may need an async runtime; if so it stays inside the push module.

**D7. Provider trait.** The trait starts with one method, `show`, which both compose `ps` and containerlab `inspect` fit. Containerlab (step 9) is the second implementation and the real test.
- The shortcut is a concrete compose type now and extract the trait at step 9. That is cheaper, but the "provider idea" stays unproven until then.
- The risk of the trait now is that step 9 reshapes it. The reviewer must refuse extra methods in step 1.
- A compose stack is services; a containerlab lab is nodes and links. Do not force both into the same noun.

**D8. One binary, `nobs`.** Groups `docker`, `containerlab`, `lab`, `utils`, `setup`, `sonda`, and `<workshop> <command>`. The old command names stay. Flags are the union: `--compose` and `--scenario`. The network name default comes from the layout. Exit codes are passed through; a usage error is 2.

**D9. Locator file `nobs.toml`.** Placed in a workshop or lab folder. The nearest file above the current folder wins. The flag `--dir` and the variable `NOBS_DIR` override.

```
Before:  Python code in each plugin; _repo_root() guesses four folders up
After:   workshops/autocon5/nobs.toml
           kind = "workshop"
           name = "autocon5"
           compose_file = "docker-compose.yml"
           workflow_label = "autocon5_quarantine_bgp"
```
- The alternative is a global list in `~/.config/nobs`. It breaks in cloud sessions that start empty.

**D10. Sonda defaults differ on purpose.** `nobs sonda` uses 8080. Workshop commands keep 8085 from their `.env` so printed guide outputs stay true.

**D11. Test strategy.** A fake Runner for argument lists, a stub HTTP server for the clients, a "surface" test (S3), and real runs where the Mac allows. Old pytest cases are references for the cascade builders. A test must fail when the behaviour it names is removed.

**D12. Comment discipline.** No comments by default, one short line at most (rule `workbench-80`).

## 6. Departures from what was asked

- Exit codes now follow the child program. The old tools returned 0. This is a deliberate fix. It was decided in the answers file (labcli item 3), and the requester has it.
- The labcli example lab is a test fixture only (answers file).
- The program name changes from `labcli` and `netobs` to `nobs` unless Q5 says otherwise. Scripts must change the first word. Not yet accepted.
- The screen steps (14 and 15) come after the project file and the SSH push (12 and 13). The requirements say "command line first, the screen right after", so this follows them. Move step 14 earlier if David wants the Sonda screen sooner. No departure is needed for that.
- The workshops' switch to the Rust `nobs` is outside this plan. The repository stays private and the workshops repository is public, so a public workflow cannot install `nobs` yet (Q4). Not yet accepted.

## 7. Questions for the requester

- Q1. Is a Linux host or cloud session available for the real containerlab and smoke runs, and who supplies the cEOS image (an Arista account is needed)? Default: yes, one is available. David supplies the image as a local tar. Until then, containerlab steps are checked by argument lists only and reported as untested on a real node.
- Q2. How is nobs shipped? Default: one static binary per platform (macOS arm64, Linux x86_64 and aarch64), built from source for now. No package and nothing published.
- Q3. Does the plan accept the locator file `nobs.toml` as in D9? Default: yes, nearest file above the current folder wins.
- Q4. Where does the smoke run happen while the repository is private? Default: a script in this repository runs the same sequence locally on a Linux host. The workshops' own workflow changes only after David decides to publish `nobs`.
- Q5. Do the program names `labcli` and `netobs` stay as aliases? Default: no. Only `nobs`, and a table shows `labcli X` becomes `nobs X`.
- Q6. How is the screen started? Default: `nobs tui`. Plain `nobs` prints help.
- Q7. Is an internal `nobs-core` crate (`publish = false`) acceptable, given "no library crate in the first version"? Default: yes. It is not a supported library for other tools.
- Q8. What should `nobs setup` do without `uv`? Today it runs `uv sync --all-packages`. Default: preflight plus the workshop bootstraps only, until INVESTIGATE finds that the host needs `uv`.

DECISION NEEDED: Is a Linux host or cloud session available for the real containerlab and smoke runs, and who supplies the cEOS image? Default: yes, one is available, and David supplies the image as a local tar; until then containerlab steps are checked by argument lists only.
DECISION NEEDED: How is nobs shipped? Default: one static binary per platform (macOS arm64, Linux x86_64 and aarch64), built from source for now, no package, nothing published.
DECISION NEEDED: Is the locator file `nobs.toml` in the workshop or lab folder, with the nearest file above the current folder winning, accepted? Default: yes.
DECISION NEEDED: Where does the smoke run happen while the repository is private? Default: a script in this repository runs the same sequence locally on a Linux host, and the workshops' own workflow changes only after David decides to publish nobs.
DECISION NEEDED: Do the program names `labcli` and `netobs` stay as aliases? Default: no, only `nobs`, with a table that maps `labcli X` and `netobs X` to `nobs X`.
DECISION NEEDED: How is the screen started? Default: `nobs tui`, and plain `nobs` prints help.
DECISION NEEDED: Is an internal `nobs-core` crate (`publish = false`) acceptable, given "No library crate in the first version" in docs/plan/answers-2026-11-04.md? Default: yes, it is not a supported library for other tools.
DECISION NEEDED: What should `nobs setup` do about `uv sync --all-packages`? Default: run preflight and the workshop bootstraps only, and skip `uv` until the investigation shows the host needs it.
INVESTIGATE: How does containerlab run on this Mac (macOS 27.2, arm64, OrbStack 2.2.5), for example inside a Linux container, and does a cEOS image exist for arm64? Sources: containerlab's documentation on macOS (the discovery must find the page), OrbStack's documentation, `reference/labcli/example/containerlab/lab.clab.yml`
INVESTIGATE: What is the effective precedence between the shell environment, `.env` and `.setup.env` in labcli, netobs and the workshops' tool, shown by running each in a temporary folder? Sources: `reference/labcli/labcli/base.py`, `reference/network-observability-lab/netobs/main.py`, `reference/workshops/packages/nobs/src/nobs/lifecycle/env.py`
INVESTIGATE: What do the Prefect flows, the webhook and the AI step need from the host, and does `try-it --auto` depend on anything beyond HTTP? Sources: `reference/workshops/workshops/autocon5/automation/flows.py`, `reference/workshops/workshops/autocon5/automation/serve.py`, `reference/workshops/workshops/autocon5/src/autocon5_workshop/workshop_sdk.py`, `reference/workshops/workshops/packt/src`, `reference/workshops/workshops/autocon5/webhook/app/main.py`
INVESTIGATE: What are the exact steps, waits and timeouts of the workshops' smoke job, and how does it install `nobs`? Sources: `reference/workshops/.github/workflows/validate.yml`
INVESTIGATE: Do the workshop stack (22 services) and the Sonda image run on arm64 under OrbStack, and how much memory do they need? Sources: `reference/workshops/workshops/autocon5/docker-compose.yml`, `reference/workshops/workshops/packt/docker-compose.yml`, `reference/sonda/docs/site/docs/deploy/docker.md`
INVESTIGATE: Which Rust crates are maintained and licence-acceptable for YAML, Jinja-style templates, HTTP and SSH, and does any SSH crate support the prompt handling that config push needs? Sources: https://crates.io, https://docs.rs
INVESTIGATE: Are the autocon5 and packt plugin sources the same apart from the differences listed, and what do the 11 Sonda catalog files and `GET /scenarios/{id}` contain? Sources: `reference/workshops/workshops/autocon5/src`, `reference/workshops/workshops/packt/src`, `reference/workshops/workshops/autocon5/sonda/catalog`, `reference/sonda/sonda-server/src/routes/scenarios.rs`
HANDOFF: Step 1, the shape: a Cargo workspace with `nobs-core` and `nobs-cli`, the Runner seam with a fake, a one-method Provider trait with the compose provider, and `nobs docker ps` with exit-code passthrough, checked by the six project checks and one real run against a compose fixture on the Mac
