# Discovery: labby

Read on 2026-11-04 by a discovery session (read-only) from the copy under `reference/`. Facts come from reading the code and the documents; nothing was run.

# What labby does today (discovery for the nobs plan)

**Result:** Labby 0.2.0 is a Python command line tool with 39 leaf commands. It controls one kind of lab system, a GNS3 server, through the `gns3fy` library. Its core works: list, create, start, stop and delete projects, nodes and links. Around that core are a declarative `build` flow and console and SSH configuration pushes for Arista EOS and Cisco IOS. Several parts are half-built or broken, and the README promises a few things the code does not do. I only read files and ran no command, so every "looks broken" below comes from reading code, not from running it.

## Rules

`netpanda-ai-workbench rules, version 0.12.0, file workbench-00-about.md`

All ten rule files I could see carry the same version, 0.12.0, so there is no version disagreement. The sections listed in `.claude/rules/workbench-00-about.md` are all present. I read `.claude/profiles/software.md` (version 0.12.0) at the path the architect confirmed.

Three departures from the task:
- **Test file read:** The task allowed file names only for `reference/labby/tests`. I opened `reference/labby/tests/test_settings.py` and read its contents. Treat what I say about tests as coming from a lead, and use only the file-name facts if you want to stay strictly within scope.
- **Search tool:** I used the search tool on `reference/labby/labby` to check which functions are called. That is reading, not a command.
- **Images not seen:** I did not see the README screenshots (`imgs/`). No named source contains them.

## Known facts

### The package and how it is built

- **Version and age:** The version is 0.2.0 (`reference/labby/pyproject.toml`). The last CHANGELOG entry is v0.2.0, dated 2022-05-30 (`reference/labby/CHANGELOG.md`). The task says the last change was 2023-04-10. `pyproject.toml` pins `pytest ^7.3.0` and `pydantic ^1.10.7`, which suggests edits after the changelog stopped, but I cannot confirm the date from the sources.
- **Libraries:** `typer` (command line), `rich` (output), `pydantic` v1 (settings), `toml`, `PyYAML`, `Jinja2` and `netaddr`. The lab-system client is `gns3fy >=1.0.0rc1`, a pre-release. Device access uses `nornir` 3.3 with `scrapli` pinned to 2020.10.10 and `nornir-scrapli` pinned to 2020.11.1 (`pyproject.toml`).
- **Python:** `python = "^3.8.1"`, but the classifiers also list 3.7 (`pyproject.toml`).
- **Entry points:** `labby = labby.main:app` and `tasks = tasks.main:app` (`pyproject.toml`). No `tasks` module appears in the sources.
- **Declared dependencies:** `nornir_utils` and `python-dotenv` are imported (`reference/labby/labby/commands/build.py`, `reference/labby/labby/main.py`) but not declared directly in `pyproject.toml`.

### How it talks to the lab system

- **The call chain:** `GNS3Provider` builds a `gns3fy.server.Server(url, user, cred, verify, timeout, retries)` (`reference/labby/labby/providers/gns3/provider.py`). Projects, nodes, links and templates are wrapped around `gns3fy` objects (`Project`, `Node`, `Link`, `Template`, `Port`) in `reference/labby/labby/providers/gns3/*.py`. Labby never calls the GNS3 REST API itself.
- **Console access:** The console path opens a Telnet connection to the GNS3 server host on the node's console port, using `scrapli` (`reference/labby/labby/providers/gns3/console_provisioner.py`).
- **SSH access:** The SSH path goes through nornir and `scrapli` (`reference/labby/labby/nornir/plugins/inventory/labby.py`).
- **External programs:** `connect node` shells out to the system `ssh` or `telnet` programs with `os.system` (`reference/labby/labby/commands/connect.py`).
- **Browser:** `run project launch` opens the GNS3 web UI at `<server>/static/web-ui/server/1/project/<id>` (`reference/labby/labby/providers/gns3/project.py`).

```
 labby command (typer)
        |
        v
 config.get_provider() ---> GNS3Provider ---> gns3fy ---> GNS3 server (REST)
        |                        |
        |                        +--> state file (JSON, labby's own extra data)
        |
        +--> node.apply_config / get_config -----> nornir + scrapli SSH --> device mgmt IP
        +--> node.bootstrap / *_over_console ----> scrapli Telnet --------> GNS3 host:console port
        +--> connect node -----------------------> system ssh / telnet
```

### Environments and providers

- **Hierarchy:** An environment is a named group in the config file. Each environment holds one or more providers and names the active one (`reference/labby/README.md`, `reference/labby/labby/config.py`).
- **Declared kinds:** `kind` accepts `gns3`, `eve_ng` and `vrnetlab` (`config.py`). `labby init` offers the same three kinds (`reference/labby/labby/main.py`).
- **Supported kinds:** `register_service` raises `NotImplementedError` for anything except `gns3` (`reference/labby/labby/providers/__init__.py`). The README says GNS3 is the only supported provider (`README.md`).

### Configuration file

- **Name and search order:** The file is `labby.toml`. Labby looks first in the current directory, then in `~/.config/labby/labby.toml` (`main.py`, `config.py`, `README.md`).
- **Selection options:** `-c/--config-file` (env `LABBY_CONFIG`) picks the file. `-e/--environment` (env `LABBY_ENVIRONMENT`) and `--provider` (env `LABBY_PROVIDER`) pick the environment and provider. Also `--verbose` and `--version`. The names `LABBY_ENVIRONMENT_NAME` and `LABBY_PROVIDER_NAME` are also read (`config.py`).
- **`.env` file:** A `.env` file is loaded with `load_dotenv()` (`main.py`).
- **Shape** (`config.py`, `reference/labby/example/labby.toml`):

```toml
[main]
environment = "default"          # active environment
# state_file = "..."             # optional, may be ${ENV_VAR}

[environment.default]
provider = "home-gns3"           # active provider
description = "Home lab environment"
# meta = { ... }                 # free-form, shown by `config environments`
# [environment.default.nornir_runner]  plugin, options  -> parsed, never used (see conclusions)

    [environment.default.providers.home-gns3]
    kind = "gns3"
    server_url = "http://gns3-server:80"
    verify_cert = false          # example writes "false" as a string
    # user, password (password may be ${ENV_VAR}), timeout (5), retries (2)
```

- **Environment variables in values:** Only `provider.password`, `main.state_file` and the project `mgmt_creds` user and password accept `${VAR}`. `server_url` and `user` do not (`config.py`, `project_data.py`). The CHANGELOG says "but only some".

### State file

- **Why it exists:** The GNS3 server cannot store labby's extra attributes, so labby keeps them in a JSON file (`README.md`, `reference/labby/labby/state_file.py`).
- **Location:** The default is `.labby_state.json` next to whichever config file was loaded. It can be overridden by `main.state_file` (`config.py`). The README says it is always `$HOME/.config/labby/.labby_state.json`.
- **Shape** (`state_file.py`):

```json
{ "<environment>": { "<provider>": { "projects": {
    "<project>": {
      "labels": ["..."],
      "nodes": { "<node>": { "labels": [], "mgmt_port": "Gi1", "mgmt_addr": "192.168.0.16/24",
                             "config_managed": true, "template": "...", "net_os": "cisco_ios",
                             "model": "...", "version": "..." } },
      "links": { "<link name>": { "labels": [] } }
} } } } }
```

- **Key name:** The README lists the node address as `mgmt_ip`, but the file stores `mgmt_addr`.
- **How it is used:** The file is read on every lookup and rewritten whole with `write_text` (no lock, no atomic write). It is written on create, update and delete of projects, nodes and links.

### Projects, nodes, templates, links

- **Project:** Maps to a GNS3 project and carries labels from the state file. Searching for a project that is closed opens it. Only the `get` commands close it again afterwards (`to_initial_state`) (`project.py`, `commands/get.py`).
- **Node:** Maps to a GNS3 node. Its fields include `mgmt_port`, `mgmt_addr` (must be `ip/prefix`), `config_managed`, `net_os`, `model` and `version` (`providers/gns3/node.py`, `models.py`).
- **Template:** Maps to a GNS3 template. If the template name has four words, labby derives the node's `net_os`, `model` and `version` from it (`providers/gns3/node.py`):
  - Pattern: `<vendor> <os> <model> <version>`.
  - Example: `Arista EOS vEOS 4.25.0FX` gives `arista_eos`, `veos`, `4.25.0FX`.
  - Names of any other shape get no derived values, and then bootstrap and config pushes refuse to run.
- **Link:** Maps to a GNS3 link between two node ports. Supported filters are `frequency_drop`, `packet_loss`, `latency`, `jitter`, `corrupt` and `bpf_expression` (`commands/create.py`, `commands/update.py`).

### Command inventory

Global options come first: `labby [--verbose] [--version] [-c FILE] [-e ENV] [--provider NAME] COMMAND`. `-p/--project` has env `LABBY_PROJECT`. `-na/-pa/-nb/-pb` mean node A, port A, node B and port B. Source: `reference/labby/labby/main.py` and `reference/labby/labby/commands/*.py`.

| Command | Inputs |
|---|---|
| `init` | `--global`. Prompts for environment name, description, provider name, kind (gns3, vrnetlab, eve_ng) and URL, then asks to save. Does not ask for user, password or `verify_cert`. |
| `config show` / `edit` / `environments` | None. `edit` opens the file in the system editor. `environments` draws a panel layout of all environments and providers. |
| `get project list` | `-f status\|auto_start\|auto_open\|auto_close`, `-v VALUE`, `-l LABEL` (repeatable) |
| `get project detail` | `PROJECT` |
| `get node list` | `-p`, `-f node_type\|category\|status`, `-v`, `-l` |
| `get node template-list` | `--nfilter`, `--value` |
| `get node template-detail` | `-t TEMPLATE` |
| `get node detail` | `NODE`, `-p`, `-o` (show properties) |
| `get node config` | `NODE`, `-p`, `-c` (console), `-u`, `-w`, `-s FILE` (save) |
| `get link list` | `-p`, `-f`, `-v`, `-l` |
| `get link detail` | `-p -na -pa -nb -pb` |
| `start project` | `PROJECT`, `--start-nodes one_by_one\|all`, `--delay 10` |
| `start node` / `restart node` / `stop node` | `NODE`, `-p` |
| `stop project` | `PROJECT`, `--stop-nodes/--no-stop-nodes` (default on) |
| `create project` | `NAME`, `--label` |
| `create node` | `NAME`, `-p`, `-t TEMPLATE`, `--mgmt-port`, `--mgmt-addr`, `--config-managed/--no-config-managed`, `--net-os`, `--model`, `--version`, `--label` |
| `create link` | `-p -na -pa -nb -pb`, `--filter-type`, `--filter-value`, `--label` |
| `delete project` / `node` / `link` | `PROJECT`, or `NODE -p`, or `-p -na -pa -nb -pb`. No confirmation prompt. |
| `update project attr` | `ATTR VALUE -p`, `-b`/`-i`/`-f` (parse value as bool, int or float) |
| `update project labels` | `"a,b" -p` (replaces the labels) |
| `update node attr` | `ATTR VALUE -n NODE -p`, `-b`/`-i`/`-f` (node labels are appended, not replaced) |
| `update node template` | `ATTR VALUE -t TEMPLATE`, `-b`/`-i`/`-f` |
| `update link filter` | `FILTER VALUE -p -na -pa -nb --por-b`, `-b`/`-i`/`-f` (the port-B long option is misspelled `--por-b`) |
| `build project` | `-f labby_project.yml`, `-u`, `-w`, `--boot-delay 5`, `--delay-multiplier 1`, `--force`. Asks yes/no before the bootstrap and config phases. |
| `build topology` | `-f` |
| `build bootstrap` | `-f`, `-u`, `-w`, `--boot-delay`, `--delay-multiplier`, `--render-only` |
| `build configs` | `-f`, `-m MODEL`, `-n NET_OS`, `--name`, `-s` (silent). `-n` is declared for both `--net-os` and `--name`. |
| `run project launch` | `PROJECT` (opens the browser) |
| `run node bootstrap` | `NODE -p`, `-r`/`-ro FILE` (render only), `--boot-delay`, `-c FILE` (own bootstrap config), `-u`, `-w`, `--delay-multiplier`, `--mgmt-gw` |
| `run node config` | `NODE -p -t TEMPLATE -v VARS.yml`, `-r`/`-ro`, `-u`, `-w`, `-c` (console), `--delay-multiplier` |
| `run project nodes-save` | `-f`, `-m`, `-n`, `--name`, `-b DIR` (backup), `-s` |
| `run project node-configs` | `-f`, `-m`, `-n`, `--name` |
| `connect node` | `NODE -p`, `-u`, `-c` (Telnet console instead of SSH) |

### Declarative project file (`labby_project.yml`)

Source: `reference/labby/docs/BUILD_NETWORK_LABS.md`, `reference/labby/labby/project_data.py`, `reference/labby/example/bgp-example/labby_project.yml`.

- **`main`:** `name`, `description`, `contributors`, `version`, `labels`, `template` (the Jinja2 config template), `mgmt_network` (`network`, `gateway`, optional `ip_range`) and `mgmt_creds` (`user`, `password`).
- **`nodes_spec`:** A list of entries with `template`, `nodes` (a list of names), `net_os`, `mgmt_port`, `labels`, `config_managed`, and an optional `bootstrap_config_template`.
- **`links_spec`:** A list of `{node, links: [{port, node_b, port_b, filter?, labels?}]}`.
- **`vars`:** Free-form data given to the config template. The examples use `defaults`, `groups` and `hosts`. The example `main.j2` includes `<net_os>.conf.j2` and covers interfaces, OSPF, BGP, SNMP and syslog for Arista EOS and Cisco IOS.
- **Others:** An optional `extra_properties` section is passed to project creation.
- **Management addresses:** Labby assigns management addresses in order from the range, one per node.

### Device support

Source: `console_provisioner.py`, `nornir_tasks.py`, `commands/run.py`.

| Capability | Arista EOS | Cisco IOS | Cisco NX-OS | Cisco XR | Juniper |
|---|---|---|---|---|---|
| Bootstrap template file | yes | yes | yes | no | no |
| Console bootstrap | yes | yes | yes | no ("not supported") | no ("not supported") |
| Console config or command | yes | yes | no | no | no |
| `show run` and `wr mem` over SSH | yes | yes | yes | no | no |

The `DeviceTypes` list in `run.py` still names `cisco_xr` and `juniper_junos`.

### Tests (file names are the allowed evidence)

- **What exists:** `reference/labby/tests/` holds one test module, `test_settings.py`, and four data files: `data/labby.toml`, `data/invalid_labby.toml`, `data/labby_env_var.toml` and `data/labby_project.toml`.
- **What is covered:** The names point to configuration loading only. Nothing in the file list covers commands, providers, the state file, console provisioning or the build flow.
- **What the content showed:** Reading `test_settings.py` (outside the file-name-only allowance, see Rules) shows that most tests of loading a config are marked `skip` ("needs to have deeper look"). Only the path helpers, the TOML load and save, and the invalid-TOML test are active.

### What the README and docs promise

Source: `README.md`, `docs/NODE_CONFIGURATION.md`.

- **Status:** The README says "Under Construction" and the console bootstrap "might need some more love". The docs hold the placeholders "(*insert feature here*)" and "Full Project configuration: TBC". The README's contents list says "5.1" for sections that are numbered 4.1 and 4.2, so those anchors are broken.
- **Not found in the code:** There is no `labby template` or `labby link` top-level command. Templates can only be listed, shown and updated. `GNS3Provider.create_template` exists but nothing calls it.
- **No project check:** There is no command that checks a project against the file and reports differences.
- **Doc says one thing, code another:** `docs/BUILD_NETWORK_LABS.md` and the example project use the link key `filter`. `build_topology` reads `filters` (`commands/build.py`), so link filters in a project file are silently ignored. The unused `get_project_link_spec` in `config.py` reads `filter`.
- **Missing link:** The README points to `imgs/` screenshots and a Docker image `davidban77/labby:v0.2.0-py3.8`. Neither can be checked from the sources.

## Reasoned conclusions

Conclusions 1 to 4 and 9 rest on code reading only and need a real run to confirm.

1. **Broken or likely broken (from reading):**
   - `run project nodes-save` and `run project node-configs` call `sync_project_data`, which does `IPNetwork(project_data.mgmt_network)` on a dictionary (`project_data.py`). That should fail.
   - `ProjectData.check_mgmt_ips` calls `.iter_hosts()` on a string when `ip_range` is absent (`project_data.py`).
   - `build_topology` uses `mgmt_addr == mgmt_ips[index]` where it means `=` (`commands/build.py`). A node that lands on the gateway address gets no management address.
   - `bootstrap_config_template` passes the file itself as the template folder (`commands/build.py`).
   - `get node config --save` writes a nornir `Result` object, not text (`commands/get.py`, `providers/gns3/node.py`).
2. **Credentials are hardcoded for SSH.** The nornir inventory always uses `netops` / `netops123` (`nornir/plugins/inventory/labby.py`). The `--user` and `--password` options of `get node config` and `run node config` are accepted but not used on the SSH path. The inventory also forces old SSH key-exchange algorithms. Anything nobs keeps for SSH must not copy this.
3. **Dead or unused code:** `load_project`, `ProjectSettings`, `NodeSpec`, `LinkSpec`, `NornirInventory` and `PROJECT_SETTINGS` in `config.py` are only referenced by themselves, and `tests/data/labby_project.toml` is a lead for that unused TOML project format. The `nornir_runner` environment setting is parsed but `init_nornir` hardcodes 100 threaded workers (`project.py`). `templates/base_config.toml.j2` is unused and describes a different schema. `provider_header`, `flatten`, `mergedicts` and `delete_nested_key` in `utils.py` have no callers. `config.py` holds a large commented-out block, and `main.py` a commented-out project step in `init`.
4. **Duplicated or inconsistent code:** `dissect_gns3_template_name` exists twice, in `providers/gns3/node.py` and `providers/gns3/template.py`. `config_task` exists in both `node.py` and `nornir_tasks.py`. Node labels append on update while project labels replace. `stop project --stop-nodes` has a help text copied from the start command. Delete commands never ask for confirmation.
5. **The project-file format and the template-name convention are what users depend on.** Existing examples and anyone's lab use `labby_project.yml` (sections above), the four-word template naming, and the `labby.toml` shape. These are the user-visible contracts. The state file is a private cache, so keeping its format matters only if nobs must read old state files.
6. **GNS3 is the only provider that ever worked.** `eve_ng` and `vrnetlab` are placeholders. The abstract classes in `models.py` (`LabbyProvider`, `LabbyProject`, `LabbyNode`, `LabbyLink`) show the intended multi-provider shape, but only one implementation exists.
7. **What is least likely to be lost by accident, in terms of daily use:** the declarative `build` flow (topology, then bootstrap, then configs), start and stop of single nodes or whole projects with a delay between nodes, per-node console and SSH configuration with template plus variables, link filters (packet loss, jitter and so on), labels, and the web-UI launch.
8. **Age risk:** `scrapli` is pinned to a 2020 release, `gns3fy` is a pre-release, and the console code uses Telnet through the older `scrapli` transport. The Telnet library in Python's standard library was removed in Python 3.13 (my own knowledge, not from a source), so labby may not run on current Python at all. I did not check this.
9. **Some behaviour is a side effect, not a design:** opening a closed project on any lookup, `time.sleep(2)` after nearly every call, and `connect node` building a shell string from the user name. Decide per item whether nobs repeats it.

## Open questions

- **GNS3 API generation:** The sources do not say which GNS3 server versions labby works with. The code strips `/v<digit>` from the base URL (`project.py`). I did not read `gns3fy`'s own documentation (a lead).
- **Which of the 39 commands do users actually use?** The code cannot tell me. This decides what nobs must keep. The `tasks` script in `pyproject.toml` has no module in the sources, so I cannot say what it was for.
- **Is the SSH credential hardcoding a known limitation or an oversight?** The README is silent.
- **Where the README and the code disagree about the state file** (see the status lines): which is the intended behaviour?
- **Leads not followed:** the `tasks` module and any Dockerfile, GitHub workflows, `imgs/` screenshots, `tests/data/*` contents, and `gns3fy`'s own repository (linked from the README).

DECISION NEEDED: Which commands must nobs keep from day one? Default: the core set (list, detail, create, delete, start, stop and restart for projects, nodes and links; templates list and detail; labels; link filters), the `labby_project.yml` build flow, node config push and web-UI launch. Everything else is "decide later".
DECISION NEEDED: Does nobs read labby's existing `labby.toml`, `labby_project.yml` and `.labby_state.json`, or may it use new formats? Default: read `labby.toml` and `labby_project.yml` as they are, and treat the state file as a rebuildable cache.
DECISION NEEDED: Should the commands that look broken (nodes-save, node-configs, `get node config --save`, `bootstrap_config_template`, link `filter` versus `filters`) be reproduced, fixed in nobs, or dropped? Default: record the intended behaviour from the docs, and fix.
DECISION NEEDED: How should nobs handle node console bootstrap for Cisco IOS, Cisco NX-OS and Arista EOS, which depends on fixed text prompts and timings? Default: keep the three supported systems, mark XR and Junos as not supported, and keep the "render the config so I can paste it" path.
DECISION NEEDED: Is GNS3 the only lab system that must be reachable through the old labby behaviour, with containerlab and docker compose coming from labcli? Default: yes. The requirements file leaves this open (`docs/plan/requirements.md`, "Not decided yet").
DECISION NEEDED: May I run labby, or may another explorer run it, against a real GNS3 server to confirm the "looks broken" items and to answer the API version question? Default: no, until you say so. This task was read-only.

CONFLICT: reference/labby/README.md and reference/labby/labby/state_file.py
CONFLICT: reference/labby/README.md and reference/labby/labby/config.py
CONFLICT: reference/labby/docs/BUILD_NETWORK_LABS.md and reference/labby/labby/commands/build.py
