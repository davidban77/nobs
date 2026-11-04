# Discovery: labcli

Read on 2026-11-04 by a discovery session (read-only) from the copy under `reference/`. Facts come from reading the code and the documents; nothing was run.

**Rules:** `netpanda-ai-workbench rules, version 0.12.0, file workbench-00-about.md`

All the other rule files I could see (`workbench-10` to `workbench-90`) and `.claude/profiles/software.md` carry version 0.12.0, so the versions agree. I read every source with the file tools and ran no command. I did not read `reference/labcli/tests/test_main.py`, as the task said.

Where I say "unverified", I read the code but did not run it. I judge from the code that these behaviours do not work as intended.

## Known facts

**Package basics**
- labcli is version 0.2.0. It needs Python 3.10 or newer and has two runtime dependencies: `typer>=0.12.5` and `python-dotenv>=1.0.1`. The pyproject description is the placeholder "Add your description here". (`reference/labcli/pyproject.toml`)
- The entry point is `labcli = "labcli.cli:app"`. The wheel packages the `labcli` folder. (`reference/labcli/pyproject.toml`)
- The package has five files: `cli.py`, `base.py`, `version.py`, `utils/containerlab_utils.py` and `utils/docker_utils.py`. I found no `__init__.py` in the folder listing. (`reference/labcli/labcli`)
- `rich` is imported in `base.py` but not listed in `pyproject.toml`. It probably arrives through typer. (`reference/labcli/labcli/base.py`, `reference/labcli/pyproject.toml`)

**Top level**
- `labcli --version` prints `LabCLI v<version>`. The version comes from the installed package metadata. Shell completion is switched on. (`reference/labcli/labcli/cli.py:9-33`, `reference/labcli/labcli/version.py`)
- The help text calls `banner()`, which prints an ASCII banner when the module is imported. The banner spells "labcli" with "b" for "l" in the ASCII art. (`reference/labcli/labcli/cli.py:10`, `reference/labcli/labcli/version.py`)
- There are two sub-apps: `labcli containerlab` and `labcli docker`. (`reference/labcli/labcli/cli.py:15-16`)

**`labcli containerlab` commands**

All three take one optional `TOPOLOGY` argument and a `--sudo/--no-sudo` option. `--sudo` is on by default and puts `sudo` in front of the command. The default topology is `./containerlab/lab.clab.yaml`. The file must exist at argument parsing. (`reference/labcli/labcli/utils/containerlab_utils.py`)

| Command | Runs |
|---|---|
| `deploy` | `containerlab deploy -t <topology>` |
| `destroy` | `containerlab destroy -t <topology> --cleanup` |
| `inspect` | `containerlab inspect -t <topology>` |

**`labcli docker` commands**
- Every command except `network` takes these:
  - `--compose/-c FILE`, default `./docker-compose.yml`. The file must exist.
  - `--profile/-p NAME`, which can be repeated.
  - `--verbose/--no-verbose`.
  - Zero or more `SERVICES` as positional arguments.
- The command is built as `docker compose -f FILE [--project-name N] [--profile P]... [--verbose] ACTION [extra options] [services] [command]`. If the env var `DOCKER_COMPOSE_WITH_HASH` is truthy, it uses `docker-compose` (the old standalone binary) instead. (`reference/labcli/labcli/utils/docker_utils.py:28-75`)
- A missing compose file logs "File not found" and exits with code 1. (`reference/labcli/labcli/utils/docker_utils.py:113-115`)
- Commands and what they run:

| Command | Compose action and fixed options | Extra options |
|---|---|---|
| `build` | `build` | none |
| `pull` | `pull` | none |
| `exec` | `exec` | extra argument `COMMAND`, default `bash` |
| `debug` | `up --remove-orphans` (runs in the foreground) | none |
| `start` | `up -d --remove-orphans` | none |
| `stop` | `stop` | none |
| `restart` | `restart` | none |
| `logs` | `logs` | `-f/--follow`, `-t/--tail N` |
| `ps` | `ps --all` | none |
| `destroy` | `down --remove-orphans` | `--volumes` adds `--volumes` |
| `rm` | `rm --stop` | `--volumes`, `--force` |
| `update` | `rm` with force, then `start` | `--volumes` |

- `docker network ACTION --name/-n NAME [--driver D] [--subnet S]`:
  - `ACTION` is one of `connect`, `create`, `disconnect`, `inspect`, `ls`, `prune`, `rm`.
  - It runs `docker network <action> ...`.
  - `--driver` and `--subnet` apply only to `create`.
  - The name is appended unless the action is `ls` or `prune`.
  - (`reference/labcli/labcli/utils/docker_utils.py:16-25,582-613`)

**Environment, configuration and state**
- At import, `base.py` loads `./.env` from the current folder into the environment, overriding existing values. It builds `ENVVARS` from `.env`, `.setup.env` and the process environment. The process environment wins. (`reference/labcli/labcli/base.py:13-14`)
- Every subprocess gets exactly `ENVVARS` as its environment. (`reference/labcli/labcli/base.py:64-97`)
- `run_cmd` logs the command, splits it with `shlex.split`, runs it with `check=False` and does not capture output. It then prints a green or red rule line. (`reference/labcli/labcli/base.py:87-106`)
- No command returns a failing exit code to the shell. The docker commands do not pass the subprocess exit code on, and the containerlab commands do not either. The only `typer.Exit(1)` is for a missing compose file. (`reference/labcli/labcli/base.py`, `reference/labcli/labcli/utils/docker_utils.py:113-115`, `reference/labcli/labcli/utils/containerlab_utils.py`)
- labcli keeps no state of its own. It has no config file, no database and no cache. The only inputs are the `.env` and `.setup.env` files, the environment variables, and the files named on the command line. (`reference/labcli/labcli/base.py`)

**The example folder**
- `reference/labcli/example/docker-compose.yml`:
  - It uses compose `include:` for `collectors.yml`, `databases.yml` and `visualization.yml` under `docker-compose.d/`.
  - It declares an `external: true` network called `labcli`. The network must exist before `up`.
- Services and profiles:

| File | Services | Profiles |
|---|---|---|
| `collectors.yml` | `telegraf-01`, `telegraf-02`, `logstash` | `collector`, `full` |
| `databases.yml` | `loki`, `prometheus` | `db`, `full` |
| `visualization.yml` | `grafana` | `vis`, `full` |

- Every service reads `../../.env`, and every container logs to a `gelf` driver at `udp://0.0.0.0:12201`, which is the logstash input.
- The compose files need these variables from `.env`: `GRAFANA_USER`, `GRAFANA_PASSWORD`, `SNMP_COMMUNITY`, `NETWORK_AGENT_USER`, `NETWORK_AGENT_PASSWORD`. No `.env` is in the folder listing. Several image names have `${VAR:-default}` fallbacks. (`reference/labcli/example/docker-compose.d/*.yml`, `reference/labcli/example/docker-compose.d/telegraf/telegraf-01.conf.toml`)
- Telegraf is built from `reference/labcli/example/docker-compose.d/telegraf/` (a Dockerfile with `pyproject.toml`, `uv.lock` and `routing_collector.py`).
  - It collects over SNMP, gNMI, an exec script, ping and HTTP checks. It renames fields and tags to a common schema and maps state names to numbers.
  - It outputs to stdout and to a Prometheus endpoint on :9004 (telegraf-01) and :9005 (telegraf-02).
  - Prometheus scrapes both.
- Grafana is provisioned with datasources, two dashboards (`device-health.json`, `meta-monitoring.json`) and `grafana.ini`. There are also `logstash/logstash.cfg` and `loki/loki-config.yml`. I did not read these four.
- The containerlab example, `reference/labcli/example/containerlab/lab.clab.yml`:
  - It has a lab named `lab`, an empty container prefix, and a management network `labcli` on 198.51.100.0/24.
  - It defines two Arista cEOS nodes with fixed management IPs. The image is `${CEOS_IMAGE:-ceos:image}`.
  - Each node publishes tcp/50051, 80, 443, 5900 and udp/161.
  - Two links join eth1 and eth2 of the two nodes.
- The startup configs `reference/labcli/example/containerlab/startups/ceos-01.conf` and `ceos-02.conf` configure BGP, OSPF, SNMP, gNMI, netconf and logging to 198.51.100.1:1515 over TCP. That is the host side of the `labcli` network, where logstash listens. The configs include a default user `labcli`/`labcli123`.
- The containerlab lab and the compose stack share the Docker network `labcli`. That is how Telegraf reaches `ceos-01` by name.

**Imports and tests**
- The README says labcli can be "imported into other scripts or CLI applications". (`reference/labcli/README.md:4`)
  - The code exposes `labcli.cli.app` (a Typer app) and the functions in `labcli/base.py`, `docker_utils.py` and `containerlab_utils.py`.
  - There is no documented API and no `__all__`.
  - `labcli.base.run_cmd` and `labcli.utils.docker_utils.run_docker_compose_cmd` look built for reuse. They have docstrings and return `CompletedProcess`.
  - `labcli/base.py`'s `.env` loading runs as an import side effect.
- One test file exists: `reference/labcli/tests/test_main.py`. I took no further facts from it.
- Other repo files exist but are not in the named sources: `reference/labcli/.github/workflows/ci.yml`, `devcontainer.yml` and `release.yml`, plus `.devcontainer/`. They are leads.

**Labby (only from the requirements page)**
- Labby is David's Python command line tool for GNS3 labs, and nobs replaces it. (`docs/plan/requirements.md`)
- The requirements page says nothing about labby's commands, so I made no comparison.

## Reasoned conclusions

Each rests on the code I read. None was run.

1. **The README promises a command the code does not have.** `labcli containerlab start example/containerlab/lab.clab.yml` does not exist; the commands are `deploy`, `destroy` and `inspect`. The README also uses the file name `lab.clab.yml`, while the code's default is `lab.clab.yaml`. Rests on `reference/labcli/README.md:36` against `reference/labcli/labcli/utils/containerlab_utils.py`.
2. **`labcli docker update` never restarts anything (unverified).** It checks `drms.returncode`, but `docker_rm` returns nothing. So `drms` is always `None`, and `update` always logs "update failed". Rests on `reference/labcli/labcli/utils/docker_utils.py:570-579`.
3. **No labcli command passes a failing exit code to the shell.** Scripts that call labcli therefore cannot detect a failed deploy or start. nobs must decide whether to keep or change this. Rests on `reference/labcli/labcli/base.py:64-108` and the command bodies.
4. **`labcli docker start` with no `--profile` starts nothing in the example.** Every example service has profiles, and compose skips profiled services when no profile is given. Use `--profile full` or a named profile. Rests on `reference/labcli/example/docker-compose.d/*.yml` and `reference/labcli/labcli/utils/docker_utils.py:61-62`. This is how compose normally behaves, not something I confirmed in the sources.
5. **`docker exec` with several services builds an invalid command (unverified).** `docker compose exec` takes one service, but the code joins all services and then appends the command. `exec` runs without `-T` and has no TTY handling beyond inheriting the terminal. Rests on `reference/labcli/labcli/utils/docker_utils.py:70-73,203-235`.
6. **`docker network` requires `--name` even for `ls` and `prune`.** The command is wrong for `create` when `--driver` is set: it emits `--driver=<d> ` followed by a trailing space, which is harmless but untidy. Rests on `reference/labcli/labcli/utils/docker_utils.py:588,602-613`.
7. **Environment variables may not reach containerlab when `--sudo` is on (unverified).** `sudo` normally strips the environment, so `${CEOS_IMAGE:-ceos:image}` in the topology would fall back to the default unless sudo is configured to keep the variable. Rests on `reference/labcli/labcli/utils/containerlab_utils.py` and `reference/labcli/example/containerlab/lab.clab.yml:11`.
8. **Small copy errors in the help text.**
   - `destroy` logs "Deploying containerlab topology".
   - `inspect`'s example says `show`.
   - `docker rm` examples say `netobs docker rm --scenario ...`, a leftover from the earlier tool.
   - Rests on `reference/labcli/labcli/utils/containerlab_utils.py:58,82` and `reference/labcli/labcli/utils/docker_utils.py:509-525`.
9. **What nobs must keep to avoid losing something a user relies on.**
   - The 3 containerlab commands and the 13 docker commands (12 compose commands plus `network`), with the exact compose and containerlab commands in the tables above.
   - The `--compose`/`--profile`/service/`--verbose` shape.
   - The default file names.
   - The `.env` and `.setup.env` loading.
   - The `DOCKER_COMPOSE_WITH_HASH` switch.
   - The `--sudo` default.
   - The shared `labcli` network between containerlab and compose.
   - The profile names in the example.
   - The reuse path (`run_cmd` and `run_docker_compose_cmd`).
   - Items 1 to 8 above are bugs or mismatches, not features worth keeping.

## Open questions

- Which of the 16 commands must be kept as-is, and which are dropped? Keeping everything also keeps the broken ones in conclusions 2 and 5.
- What does "imported by other tools" mean for a Rust program? Today it means a Python import. A Rust program has no such path. The option is a library crate, a scripting-friendly CLI with a stable exit code, or neither.
- Which tools import labcli today? None is named in the sources. The requirements page says the workshops repository has its own Python `nobs`, and `reference/labcli/labcli/utils/docker_utils.py` mentions `netobs` in its help text. Whether they import labcli is unknown.
- Should nobs pass on the exit code of the command it runs? Today it never does.
- Does `reference/labcli/labcli/__init__.py` exist? The listing shows none. The package may work as a namespace package, but I did not confirm that.
- `reference/labcli/example/docker-compose.d/loki/rules/` and `prometheus/rules/` are mounted by the compose files but not in the folder listing. Does the example need them to work?
- Leads I did not follow:
  - `reference/labcli/tests/test_main.py` (what it covers is unknown beyond the file name)
  - `reference/labcli/.github/workflows/ci.yml`, `devcontainer.yml` and `release.yml`
  - `reference/labcli/.devcontainer/`
  - `reference/labcli/uv.lock`
  - the example's Grafana dashboards and `logstash.cfg`
  - `reference/labcli/example/docker-compose.d/telegraf/routing_collector.py`, `telegraf-02.conf.toml` and `Dockerfile`
- The README links a Codespaces badge and a GitHub project. I did not follow them.
- Conclusions that still need checking by running the tool: 2, 5, 7 and the claim in 4 about compose profiles. I have not run labcli.

No source contains an instruction to me, so there is no `INSTRUCTION IN SOURCE` line.

DECISION NEEDED: Which labcli commands are in the first version of nobs? Default: all 3 containerlab commands and all docker compose commands except `update` and `exec`, which are broken today; `update` and `exec` are rebuilt properly.
DECISION NEEDED: Must nobs keep the exact `labcli containerlab ...` and `labcli docker ...` command names for existing scripts? Default: yes, as aliases next to any new names, until David says otherwise.
DECISION NEEDED: Does nobs pass the exit code of the command it runs back to the shell? Default: yes. This changes labcli's behaviour on purpose, so it is a change David should accept.
DECISION NEEDED: Is there anything in "imported by other tools" that nobs must still offer, and by which path (library crate, CLI, or neither)? Default: a stable CLI with exit codes only, no library crate.
DECISION NEEDED: Should nobs keep reading `./.env` and `.setup.env` with the same precedence, including `DOCKER_COMPOSE_WITH_HASH` for the old `docker-compose` binary? Default: keep `.env`, `.setup.env` and the precedence; drop `DOCKER_COMPOSE_WITH_HASH` unless a user needs the old binary.
DECISION NEEDED: Is the example lab (cEOS plus the Telegraf/Prometheus/Loki/Grafana stack) part of what nobs ships, or only a fixture for tests? Default: fixture for tests only.
DECISION NEEDED: How are the overlaps with labby decided? The requirements page gives no labby detail, so the architect needs a separate discovery of `reference/labby` first. Default: no overlap is assumed until that discovery is done.

(No `MISSING:` or `CONFLICT:` lines apply. The one disagreement I found, README versus code, is a statement the code does not meet, which the skill treats as a finding, not a conflict.)
