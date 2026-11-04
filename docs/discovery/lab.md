# Discovery: lab

Read on 2026-11-04 by a discovery session (read-only) from the copy under `reference/`. Facts come from reading the code and the documents; nothing was run.

The ask is to replace a Python tool that wraps three other command-line tools and a few REST calls, so nobs has to run those same tools and cannot be self-contained. I read the netobs code, the compose files, both README files, the env examples, the playbook entry point, `ch5` and a sample of `batteries-included`. I ran nothing and did not check how the lab behaves live.

I read about half of the `batteries-included` folder and `containerlab`, not every file. I did not read the Loki config, recording and alert rules, dashboards, `nautobot_config.py`, `telegraf-02`, `routing_collector.py`, the webhook `api.py` and `config.py`, the Dockerfiles, the four Prefect scripts (`devnet-demo.py`, `event-automation.py`, `observer.py`, `rca.py`) or `ceos-02.conf`. I also did not read the other setup playbooks and tasks, which are leads (see open questions).

## Rules

netpanda-ai-workbench rules, version 0.12.0, file workbench-00-about.md

The other loaded files (`workbench-10` through `workbench-90`) and `.claude/profiles/software.md` carry the same 0.12.0 line, so the versions agree. All ten sections are visible: About these rules; Approval boundaries; Think before acting; Keep changes small; Verify, then report what happened; Delegating work; Review standards; Writing standard; Code comments; Tooling. Nothing is missing.

## Known facts

### What netobs is

- `reference/network-observability-lab/pyproject.toml` defines a Python package, `netobs` version 0.0.17, built with Typer, Rich and Netmiko, with the entry point `netobs.main:app`.
- The whole tool is one file, `reference/network-observability-lab/netobs/main.py` (1865 lines). The folder holds no other file, and I saw no `__init__.py`.
- It shells out to external programs for most work and uses REST and SSH for the rest:

```
 netobs (one Python file)
   |-- subprocess --> containerlab deploy/destroy/inspect  (optional sudo)
   |-- subprocess --> docker compose --project-name netobs -f chapters/<scenario>/docker-compose.yml ...
   |-- subprocess --> docker network create/rm/ls ...
   |-- subprocess --> ansible-galaxy / ansible-playbook setup/*.yml
   |-- REST (requests) --> Nautobot  http://localhost:8080  (Token auth)
   |-- REST (requests) --> Prefect   http://localhost:4200/api
   |-- SSH (netmiko)   --> cEOS / Nokia SR Linux devices by hostname
```

- It does not call Prometheus, Loki, Grafana or Alertmanager. `prometheus-api-client`, `prefect`, `openai`, `jmespath` and `ipython` are declared dependencies, but `main.py` imports none of them (`pyproject.toml:14-25`). They are probably there for the chapter scripts, which I did not read.

### Every command

All paths are relative to `reference/network-observability-lab/netobs/main.py`. Most `docker` commands and all `lab` commands take `--scenario/-s`, or `LAB_SCENARIO`, from a fixed list of 18 names. `docker network` and `lab purge` take none.

| Command | Inputs | What it runs |
|---|---|---|
| `containerlab deploy` | topology (argument, default `./containerlab/lab.yml`), `--sudo` (default **true**, env `LAB_SUDO`) | `containerlab deploy -t <topo>` |
| `containerlab destroy` | same | `containerlab destroy -t <topo> --cleanup` |
| `containerlab inspect` | same | `containerlab inspect -t <topo>` |
| `docker build` | scenario, services..., `--verbose` | `compose build` |
| `docker exec` | service, command (default `bash`), scenario | `compose exec` |
| `docker debug` | scenario, services... | `compose up --remove-orphans` (foreground) |
| `docker start` | scenario, services... | `compose up -d --remove-orphans` |
| `docker stop` | scenario, services... | `compose stop` |
| `docker restart` | scenario, services... | `compose restart` |
| `docker logs` | scenario, services..., `-f`, `-t/--tail` | `compose logs` |
| `docker ps` | scenario, services... | `compose ps` |
| `docker destroy` | scenario, services..., `--volumes` | `compose down --remove-orphans [--volumes]` |
| `docker rm` | scenario, services..., `--volumes`, `--force` | `compose rm --stop ...` |
| `docker network` | action (connect/create/disconnect/inspect/ls/prune/rm), `-n` name (default `network-observability`), `--driver`, `--subnet` (default `198.51.100.0/24`) | `docker network <action>` |
| `lab deploy` | scenario, `--topology`, `--network-name` (env `LAB_NETWORK_NAME`), `--subnet` (env `LAB_SUBNET`), `--sudo` (default **false**) | network create, then containerlab deploy, then compose start |
| `lab destroy` | scenario, `--topology`, `--sudo` | `compose down --volumes`, then containerlab destroy |
| `lab purge` | `--sudo` | runs `lab destroy` for all 18 scenarios in turn, ignoring "file not found" exits |
| `lab show` | scenario, `--topology`, `--sudo` | `compose ps`, then containerlab inspect |
| `lab prepare` | scenario, `--topology`, `--sudo` | `lab purge`, then containerlab deploy, then compose start (no network create) |
| `lab update` | services... (required), scenario | `compose rm --stop --force --volumes`, then `compose up -d` |
| `lab rebuild` | services... (required), scenario | stop, build, start |
| `setup deploy` | `-v`, `-e` extra vars, scenario (default `batteries-included`), `--topology`, `--vars-topology` | writes `./keep_api_key` from `KEEP_API_KEY`; `ansible-galaxy collection install -r setup/requirements.yml`; `ansible-playbook setup/create_droplet.yml`, then `setup/setup_droplet.yml`; deletes `./keep_api_key` |
| `setup linux` | same options | installs the collections, then `ansible-playbook setup/setup_linux.yml` |
| `setup destroy` | `-v`, `-e` | `ansible-playbook setup/destroy_droplet.yml` |
| `setup show` | none | `ansible-playbook setup/list_droplet.yml` (prints the SSH command) |
| `utils load-nautobot` | `--nautobot-token` (env `NAUTOBOT_SUPERUSER_API_TOKEN`, required), `--topology`, `--extra-topology-vars` (default `./containerlab/lab_vars.yml`), `--nautobot-url` (env `NAUTOBOT_URL`, default `http://localhost:8080`) | about 15 kinds of REST POST to Nautobot (see below) |
| `utils delete-nautobot` | token, url | REST GET then DELETE of 10 object kinds |
| `utils device-interface-flap` | `--device`, `--interface` (both required; env `LAB_DEVICE`, `LAB_INTERFACE`), `--count` (1), `--delay` (5), `--platform` (`arista_eos` or `nokia_srl`) | SSH shutdown/no shutdown, repeated |
| `utils load-prefect-secrets` | prefect url (env `PREFECT_API_URL`), nautobot token, openai key, network user/password, device type, slack bot token | Prefect REST, creates or updates 6 `Secret` blocks |
| `utils delete-prefect-secrets` | prefect url | Prefect REST, deletes all `Secret` blocks |

- Sources: `containerlab` commands at `main.py:417-468`; `docker` at `main.py:476-819`; `lab` at `main.py:827-1005`; `setup` at `main.py:1060-1214`; `utils` at `main.py:1222-1864`.
- `load-nautobot` creates, in order: a role, the manufacturers Arista and Nokia with a device type each, a location type, the statuses `lab-active` and `Alerted`, a custom field `maintenance`, a location `lab`, an IPAM namespace `lab-default`, the prefixes from `containerlab/lab_vars.yml`, the management prefix, then per node a device, interfaces, IP addresses with their interface mappings, a `Management0` interface and a primary IP.
- Each device also gets a `local_config_context_data` entry `observability_intent.bgp` taken from `lab_vars.yml`. In `containerlab/lab_vars.yml` I saw only prefixes and per-node interfaces, with no `observability_intent` section.
- Setup mode `topology` and `vars_topology` defaults are the global files. The `setup` and `utils` commands do not read per-chapter topologies.

### How it starts and stops the lab

- The path through `main.py` is: `lab deploy` runs `docker network create --driver=bridge --subnet=198.51.100.0/24 network-observability`, then `containerlab deploy`, then `docker compose up -d` with the project name always `netobs`. `lab destroy` runs compose down with volumes, then containerlab destroy. `lab prepare` skips the network create.
- `containerlab/lab.yml` defines two Arista cEOS nodes, `ceos-01` (198.51.100.11) and `ceos-02` (198.51.100.12), two links (Ethernet1 and Ethernet2 between them), and the management network `network-observability` on 198.51.100.0/24. There is no name prefix.
- The image comes from `CEOS_IMAGE`, defaulting to `ceos:image`.
- The startup configs create user `netobs`/`netobs123`, SNMP community `public`, gNMI on port 50051, BGP AS 65111, OSPF 77, and syslog to host 198.51.100.1 port 1515 over TCP (`containerlab/startups/ceos-01.conf:5,15-17,44-50,53,160`).
- 198.51.100.1 is the Docker bridge gateway, i.e. the lab host. Logstash publishes port 1515 on the host. So the subnet and the host's role are baked into the device configs. The `--subnet` option can't change this without editing those files.
- Setup playbooks: `setup/setup_linux.yml` runs two plays on localhost. The first bootstraps the system (`tasks/bootstrap.yml`) and copies `example.env` to `.env` if it is absent. The second sets `ansible_python_interpreter` to `~/miniconda/bin/python` and imports `tasks/deploy.yml`.
- The setup README says this installs Docker, Containerlab, Miniconda (Python 3.12) and netobs, deploys the lab, and loads Nautobot (`setup/README.md:66`). I did not read the task files that do this.
- Every command goes through `run_cmd`, which logs the command and the result but never stops on a non-zero exit (`main.py:292-337`).

### Services the lab runs

All attach to the external Docker network `network-observability`.

`batteries-included` (`chapters/batteries-included/docker-compose.yml`) has 16 services:

| Service | Image | Host port | Role |
|---|---|---|---|
| telegraf-01, telegraf-02 | built from `telegraf.Dockerfile`, base `telegraf:1.31` | 9004, 9005 | Collect from ceos-01/02 over SNMP, gNMI, SSH (custom Python collector) and ping; expose Prometheus metrics |
| logstash | `grafana/logstash-output-loki:3.1.1` | 1515, 9600, 12201/udp | Syslog in (1515); Docker container logs in via the GELF logging driver (12201); both out to Loki |
| loki | `grafana/loki:3.1.1` | 3001 | Log store |
| prometheus | `prom/prometheus:v2.52.0` | 9090 | Scrapes the two Telegrafs; remote write receiver on; rules; 512 MB retention |
| alertmanager | `prom/alertmanager:v0.26.0` | 9093 | Routes `PeerInterfaceFlapping` to the webhook service, and critical/warning to Keep |
| grafana | `grafana/grafana:10.4.4` | 3000 | Provisioned datasources: prometheus, loki, nautobot (GraphQL), infinity; 4 plugins installed at start |
| nautobot, nautobot-redis, nautobot-postgres | `networktocode/nautobot:2.4-py3.10`, `redis:7.2-alpine`, `postgres:14` | 8080, 8443 | Source of truth |
| webhook | built from `webhook.Dockerfile` (FastAPI) | 9997 | Receives Alertmanager alerts |
| postgres, redis, prefect-server, prefect-services, prefect-worker | `prefecthq/prefect:3-latest` etc. | 4200 | Self-hosted Prefect 3, work pool `local-pool` |

- Nine of these (telegraf, logstash, loki, prometheus, alertmanager, grafana, nautobot, webhook) are tied together by a GELF log driver pointing at `udp://0.0.0.0:12201` on the Docker host, so container logs flow into Logstash and Loki.
- The Alertmanager chain is: Prometheus rule, Alertmanager route, webhook at `http://webhook:9997/v1/api/webhook`, then Prefect (`alertmanager/alertmanager.yml:19-27,35-37`; `CLAUDE.md:61-63`).
- Keep (`https://api.keephq.dev/...`) is an external SaaS and needs `keep_api_key`.
- Other documented ports: SSH to `ceos-01` or `ceos-02` as `netobs`/`netobs123` and gNMI 50051 on the devices.

### Environment variables and files expected

- `main.py:25-26`: loads `./.env` with override on, then builds `ENVVARS = {.env, .setup.env, os.environ}` and passes it to every subprocess. Everything is relative to the current directory, so netobs must be run from the repo root.
- Read directly by netobs: `LAB_SCENARIO`, `LAB_SUDO`, `LAB_NETWORK_NAME`, `LAB_SUBNET`, `DOCKER_COMPOSE_WITH_HASH`, `NAUTOBOT_SUPERUSER_API_TOKEN`, `NAUTOBOT_URL`, `LAB_DEVICE`, `LAB_INTERFACE`, `LAB_FLAP_COUNT`, `LAB_FLAP_DELAY`, `LAB_PLATFORM`, `PREFECT_API_URL`, `OPENAI_API_KEY`, `NETWORK_AGENT_USER`, `NETWORK_AGENT_PASSWORD`, `SLACK_BOT_TOKEN`, `KEEP_API_KEY`.
- Used by compose: `CEOS_IMAGE`, the `*_IMAGE` overrides, `PYTHON_VER`, `GRAFANA_USER`, `GRAFANA_PASSWORD`, `NAUTOBOT_POSTGRES_*`, and everything in `.env` (each service has `env_file: ../../.env`).
- `example.env` (66 lines) has Nautobot, Postgres and Redis values, the superuser and token, Grafana, network-agent credentials, `SNMP_COMMUNITY`, `LAB_SCENARIO`, `LAB_SUDO`, `PREFECT_URL`, `PREFECT_KEY`, `OPENAI_API_KEY`, and a commented `CEOS_IMAGE`.
- `example.setup.env` (13 lines) has `DO_API_TOKEN`, `DO_SSH_FINGERPRINT`, `DO_DROPLET_CREATION_DELAY`, `SSH_KEY_PATH`, `CEOS_IMAGE_PATH`, `NETOBS_REPO`, `NETOBS_BRANCH`.
- Files expected on disk: `containerlab/lab.yml`, `containerlab/lab_vars.yml`, `chapters/<scenario>/docker-compose.yml`, `setup/requirements.yml`, `setup/*.yml`. The cEOS image must be imported by hand (`docker import <tar> ceos:image`); I did not find any step in the sources I read that does it automatically for a local run.

### How the chapter scenarios differ

- Folders directly under `chapters/` (from the 18 compose files found): `batteries-included`, `ch3-completed`, `ch5`, `ch5-completed`, `ch6`, `ch6-completed`, `ch7`, `ch7-completed`, `ch8`, `ch8-completed`, `ch9`, `ch9-completed`, `ch12`, `ch12-completed`, `ch13`, `ch13-completed`, `webinar`, `webinar-completed`. These 18 match the `NetObsScenarios` list at `main.py:56-76` exactly. `ch3` has no skeleton, only `ch3-completed`.
- All scenarios share the single `containerlab/lab.yml` and the network name. The scenario selects only which compose file runs.
- `chapters/ch5` has 6 files: `docker-compose.yml`, `logstash/logstash.cfg`, `telegraf/script.py`, `telegraf/telegraf-01.conf.toml`, `telegraf/telegraf-02.conf.toml`, `telegraf/telegraf.Dockerfile`. It runs 3 services: `telegraf-01`, `telegraf-02`, `logstash`.
- The `logstash.cfg`, both Telegraf configs and `script.py` are **empty files (0 bytes)**. This is deliberate; the book exercise is to fill them in (`setup/README.md:272-274`).
- `ch5` mounts `script.py` into `telegraf-02` only, has no `container_name`, no GELF logging, no volumes, uses the logstash image tag `main` and no `12201`/`9600` ports.
- `chapters/batteries-included` has 16 services plus Grafana provisioning, Loki and Prometheus rules, a Nautobot config, a FastAPI webhook, a Telegraf routing collector (Netmiko + TTP, prints Influx line protocol), and four Prefect scripts (`devnet-demo.py`, `event-automation.py`, `observer.py`, `rca.py`). It is the full, finished stack, with `container_name` pinned and every image version set.
- The pattern from `CLAUDE.md:53-59` and `README.md:151-154`: each chapter has `chN` (skeleton) and `chN-completed` (reference answer).

### What the README promises that the code does not do

- `README.md:110` shows `netobs lab deploy batteries-included --sudo` with the scenario as a plain argument. In `main.py:827-840` the scenario is the option `--scenario/-s`, so that command would be rejected.
- The sample output in the README shows the compose file at `chapters/docker-compose.yml` and a service `grafana-01` with volumes named `netobs_grafana-01_data`. The code uses `chapters/<scenario>/docker-compose.yml` and the compose has `grafana`. The sample is stale.
- `README.md:106` says deploy ensures "all necessary components are up and running". Code ignores exit codes (`main.py:331-335`) and never waits for readiness. `lab_deploy` prints "deployed" even if containerlab failed.
- `README.md:102` says `utils` "enabling/disabling" an interface. The code has only `device-interface-flap`, a shutdown followed by no shutdown.
- `chapters/batteries-included/README.md:99-105` says the commands "shut down" Ethernet2 but shows `no shutdown`. Line 350 says Grafana credentials are "usually admin/admin", whereas `example.env` sets `netobs`/`netobs123`.
- `README.md:151-154` says each practical chapter has two scenarios. `ch3` has one.
- `CLAUDE.md:19` lists `netobs docker {start,stop,restart,logs,ps,exec,build,destroy}`. The code also has `debug`, `rm`, `network`. `CLAUDE.md` omits `lab prepare/purge/rebuild`, `utils delete-nautobot` and both Prefect-secrets commands.
- Docstrings that mislead: `containerlab destroy` logs "Deploying containerlab topology" (`main.py:445`); `containerlab inspect` shows `netobs containerlab show --topology` (`main.py:461`); `docker exec` shows `--command bash` where `command` is a positional argument; `setup show` shows `netobs setup list` and runs with `task_name="test"` (`main.py:1208,1214`).
- The README promises a Codespaces/devcontainer path (`README.md:1`). The `.devcontainer` folder is not a named source.

### What is tested

- `reference/network-observability-lab/CLAUDE.md:43`: "No test suite exists — scenarios are validated by deploying and exercising them manually."
- A glob for test files found none. The only workflow file is `.github/workflows/devcontainer-build.yml` (a lead; not read).
- `pyproject.toml` configures `ruff` (line length 120) and lists `black`, `flake8`, `ruff`, `pydocstyle` as dev extras.

### Unfinished, abandoned or doubtful

All paths relative to `reference/network-observability-lab/netobs/main.py`.

- `load-nautobot` bug: `device_types` is overwritten in the Arista/Nokia loop (`main.py:1260-1274`), so after the loop it holds the Nokia `SRLinux` type. Every device, including the cEOS routers, is then created with that device type (`main.py:1394`). The README does not mention this.
- `load-nautobot` is not repeatable. The first duplicate `POST` raises a `ValueError` (`main.py:185-186`), except for the custom field, which has a `try/except` (`main.py:1307-1323`).
- `delete-nautobot` issues `DELETE` on list endpoints with a body made of only the first page of a `GET` (`main.py:1523-1583`). The pagination limit is not handled, and I did not verify that Nautobot accepts this.
- `device-interface-flap` hardcodes the credentials (`netobs`/`netobs123`, or `admin`/`NokiaSrl1!`), ignoring `NETWORK_AGENT_USER` and `NETWORK_AGENT_PASSWORD` (`main.py:1608-1613`). It reaches the device by name (`ceos-02`), which depends on containerlab's host entries. Nokia SR Linux code exists, but `containerlab/lab.yml` has only cEOS nodes and the README never mentions SR Linux.
- `setup deploy` writes the secret `./keep_api_key` and deletes it only on success (`main.py:1081-1126`). If any step `Abort`s, the file stays on disk.
- `example.env` sets `OPENAI_API_KEY=<your_token>`, which is not empty, so `load-prefect-secrets` uploads the placeholder text as a real secret (`main.py:1706`).
- `KEEP_API_KEY` and `SLACK_BOT_TOKEN` are not in either example env file. `example.env` names the Prefect variable `PREFECT_URL`, while the code reads `PREFECT_API_URL`.
- `containerlab` commands default `--sudo` to true; `lab` commands default it to false (`main.py:420` vs `main.py:839`).
- `lab purge` runs 18 compose-down and containerlab-destroy cycles (`main.py:893-897`).
- The `webhook` service's check at `webhook/app/main.py:28-48` requires a Prefect Cloud URL, while the compose file points at the local `http://prefect-server:4200/api`. The mismatch is only logged as an error; the service still starts.
- The compose file bind-mounts `./alertmanager/keep_api_key`, and the README tells the user to create `alertmanager/slack_webhook`. Neither is in the folder listing.
- Left-over code: commented lines at `main.py:180,1396`, an unused `verify_ssl` argument, `requests.packages` import (deprecated), `# restart: always` in the compose file.
- `requests` and `typing_extensions` are imported by `main.py` but not declared in `pyproject.toml`.
- `Chapter 3` content is described in the README as "SNMP, gNMI, SSH, REST, eBPF"; the only lab folder is `ch3-completed`.

## Reasoned conclusions

1. **A replacement must run these external tools, not reimplement them.** The lab is driven by `containerlab`, `docker compose`, `docker network` and `ansible-playbook` commands, plus two REST APIs and SSH. (Rests on the command table and `run_cmd`.)
2. **The lab's real contract is a layout, not an API:** the repo-root working directory, `chapters/<name>/docker-compose.yml`, `containerlab/lab.yml` and its `lab_vars.yml`, the Docker network name `network-observability`, the subnet, the device names, the fixed credentials, and the project name `netobs`. `reference/network-observability-lab/CLAUDE.md:95-100` says not to change these, because the chapters depend on them.
3. **A new tool can find scenarios by listing `chapters/*/docker-compose.yml`** instead of a hardcoded list; `chapters/` has the 18 folders the enum names, so the list could be derived. (Rests on the folder list and `main.py:56-76`.)
4. **The Linux-only parts matter for David's Mac.** `containerlab` needs a Linux host, the devices send syslog to the Docker bridge gateway, the GELF driver targets the host, and the README says "Developed and tested on Debian-based Linux" (`README.md:24`). I did not verify how this behaves on macOS. The "on his own computer and in cloud sessions" statement in `docs/plan/requirements.md:18` therefore needs a decision on where the lab actually runs.
5. **The set of things to carry forward is small:** deploy, destroy, purge, show, prepare, update/rebuild, logs/exec, the flap helper. The Nautobot loader and Prefect secrets are a different kind of job: they are one-time seeding of the services (see Open questions).
6. **Because exit codes are ignored and there are no tests, the 'works' state of the old tool is unverified.** Any behaviour copied from it needs its own check. (Rests on `main.py:331-335` and `CLAUDE.md:43`.)
7. **The two README/code disagreements show the docs were not tested against the code.** Treat README command lines as intent, and `main.py` as the behaviour. (Rests on the README vs code facts above.)

Conclusions 4 and 5 need checking before the plan relies on them.

## Open questions and leads not followed

- **Leads not read** (not named, or more than one level away): `reference/network-observability-lab/setup/tasks/bootstrap.yml`, `setup/tasks/deploy.yml`, `setup/setup_droplet.yml`, `setup/create_droplet.yml`, `setup/destroy_droplet.yml`, `setup/list_droplet.yml`, `setup/requirements.yml`, `setup/inventory/*.yaml`, `.github/workflows/devcontainer-build.yml`, `.github/copilot-instructions.md`, any `.devcontainer` folder, and the other chapter folders (listed above by name only).
- Not read inside named folders: see the opening paragraph.
- Does `tasks/deploy.yml` run `utils load-nautobot` and import the cEOS image? The setup README says so. I could not confirm it.
- Does Docker, on this lab's version, create a directory when a bind-mount source like `alertmanager/keep_api_key` is missing? It does in general; the effect on Alertmanager here is unverified.
- Which of the chapter folders carry their own `containerlab/` folder? The README mentions `./chapters/webinar/containerlab/lab.yml`, but `main.py` defaults to the global `containerlab/lab.yml`.
- Which Prefect scripts in `batteries-included` need to run on the host or in a container, and whether netobs is expected to start them.
- Is the Nautobot data loader a command, a one-time seeding step, or lab-image content? And should the Device Type bug be fixed or dropped?

DECISION NEEDED: Which lab hosts must nobs support in the first version: only Debian/Ubuntu Linux (local and DigitalOcean, as the README says), or also David's Mac with the lab running in a remote or cloud Linux machine? Default: the lab runs on Linux (a droplet or cloud session); nobs on the Mac drives it over SSH or runs on the same Linux host.
DECISION NEEDED: Does nobs call the same external programs (`containerlab`, `docker compose`, `docker network`, `ansible-playbook`), or does it replace some of them (for example, no Ansible for the local Linux setup)? Default: call containerlab and docker compose, keep Ansible only for the DigitalOcean path.
DECISION NEEDED: Is the DigitalOcean path (droplet create, setup, destroy, show) in the first version? Default: out of the first version; keep only `setup linux`-style bootstrap or none.
DECISION NEEDED: Must the second-edition lab keep the current layout (`chapters/<name>/docker-compose.yml`, the one `containerlab/lab.yml`, network `network-observability`, subnet 198.51.100.0/24, `netobs`/`netobs123`), or may the chapters be reorganised? Default: keep the layout; the chapters and device configs depend on it.
DECISION NEEDED: Should nobs list scenarios by reading `chapters/` instead of a hardcoded list? Default: yes, read the folder (this removes the need to edit code for a new chapter).
DECISION NEEDED: Which of `utils load-nautobot`, `delete-nautobot`, `load-prefect-secrets`, `delete-prefect-secrets` and `device-interface-flap` come into nobs, and does nobs call the Nautobot and Prefect APIs itself or only run Python scripts? Default: carry the flap helper and the Nautobot load; leave Prefect secrets in the Python scripts for now.
DECISION NEEDED: Should the new tool stop on failed steps and wait for services to be ready (the old tool never does)? Default: yes, stop on a non-zero exit and report which step failed.
DECISION NEEDED: Is Nokia SR Linux support (code exists, no topology or docs) in or out? Default: out.
DECISION NEEDED: Is the cEOS image import a nobs task or a manual step that nobs only checks for? Default: nobs checks for the image and tells the user how to import it.

INSTRUCTION IN SOURCE: reference/network-observability-lab/CLAUDE.md
INSTRUCTION IN SOURCE: reference/network-observability-lab/README.md
INSTRUCTION IN SOURCE: reference/network-observability-lab/setup/README.md
INSTRUCTION IN SOURCE: reference/network-observability-lab/chapters/batteries-included/README.md
CONFLICT: reference/network-observability-lab/README.md and reference/network-observability-lab/netobs/main.py
