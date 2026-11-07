# nobs

A terminal program (a text user interface) for network labs and observability workshops. It replaces the labby command line tool and takes in the lab tooling of labcli, the workshops and the network observability lab.

Status: early. The `nobs docker` group (13 commands), the `nobs containerlab` group (3 commands), the `nobs sonda` group (10 commands), `nobs tui`, `nobs workshops` and `nobs <workshop> ps` work. The planning papers (requirements, findings, decisions and the plan) are not kept in this repository.

## Commands

Each `docker` command of labcli and netobs has the same name in nobs, and so does each `containerlab` command of labcli. netobs has no `containerlab` commands, so its cell is empty in those rows.

| labcli X | netobs X | nobs X |
|---|---|---|
| `labcli docker build` | `netobs docker build` | `nobs docker build` |
| `labcli docker pull` | `netobs docker pull` | `nobs docker pull` |
| `labcli docker exec` | `netobs docker exec` | `nobs docker exec` |
| `labcli docker debug` | `netobs docker debug` | `nobs docker debug` |
| `labcli docker start` | `netobs docker start` | `nobs docker start` |
| `labcli docker stop` | `netobs docker stop` | `nobs docker stop` |
| `labcli docker restart` | `netobs docker restart` | `nobs docker restart` |
| `labcli docker logs` | `netobs docker logs` | `nobs docker logs` |
| `labcli docker ps` | `netobs docker ps` | `nobs docker ps` |
| `labcli docker destroy` | `netobs docker destroy` | `nobs docker destroy` |
| `labcli docker rm` | `netobs docker rm` | `nobs docker rm` |
| `labcli docker update` | `netobs docker update` | `nobs docker update` |
| `labcli docker network` | `netobs docker network` | `nobs docker network` |
| `labcli containerlab deploy` | | `nobs containerlab deploy` |
| `labcli containerlab destroy` | | `nobs containerlab destroy` |
| `labcli containerlab inspect` | | `nobs containerlab inspect` |

In `nobs docker exec`, the words after the service are passed to the container as separate arguments, and a flag meant for the command inside the container goes after `--`.

Build the Linux binary with `cargo build --release --target aarch64-unknown-linux-musl -p nobs-cli`; it runs inside the OrbStack Linux machine `clab` and uses the Docker engine of `clab`, and the two Docker engines on the Mac (the one of OrbStack and the one inside `clab`) do not share networks.

Run the `containerlab` commands inside the Linux machine `clab`, with the Linux binary: `orb -m clab <path> containerlab deploy`, where `<path>` is the full path of `target/aarch64-unknown-linux-musl/release/nobs`. nobs runs containerlab with `sudo` unless you give `--no-sudo`. By default, sudo does not pass on the variables of your shell. So nobs reads the topology file, finds each variable it uses as `${NAME}`, and tells sudo to keep it. Set such a variable inside `clab`, for example `orb -m clab env NAME=value <path> containerlab deploy`, or in the `.env` file of the lab folder (of the current folder when you give a `TOPOLOGY` file).

## Where nobs runs from

Most commands work on one lab or one workshop. Its folder is called the lab folder here. You do not have to be inside the lab folder: nobs looks for it, starting from the folder you are in.

nobs knows three kinds of lab folder:

| Kind | How nobs knows it |
|---|---|
| A workshop, or a lab with its own settings | the folder holds a `nobs.toml` file |
| The book lab | the folder holds a `chapters/` folder and the file `containerlab/lab.yml` |
| A labcli lab | the folder holds a `docker-compose.yml` file |

nobs takes the nearest `nobs.toml` upward, else the nearest book lab, else the nearest `docker-compose.yml`. Upward means the current folder first, then its parent folder, then the parent of that, up to the top. nobs looks for one kind at a time, so a `nobs.toml` higher up wins over a `docker-compose.yml` lower down. For example, inside `chapters/ch1` of the book lab, the `docker-compose.yml` of that chapter does not make the chapter folder a labcli lab: nobs finds the book lab above it.

To name the lab folder yourself, give `--dir PATH`, or set the variable `NOBS_DIR` in the shell. `--dir` wins over `NOBS_DIR`, and an empty `NOBS_DIR` counts as not set. `nobs docker` and `nobs containerlab` then use that folder only. `nobs workshops` and `nobs <workshop>` also look in other places, and still stop on a `nobs.toml` they cannot read above the current folder (see [Workshops](#workshops)). nobs reads `NOBS_DIR` from the shell only, not from a `.env` file. `--dir` goes before a workshop name: `nobs --dir PATH nobs-t-one ps` works, but `nobs nobs-t-one --dir PATH ps` is a usage error.

These commands need a lab folder:

- a `nobs docker` command without `--compose`, with or without `--scenario`, except `nobs docker network`;
- a `nobs containerlab` command without a `TOPOLOGY` file;
- `nobs <workshop> ...` (see [Workshops](#workshops)).

These commands do not need a lab folder. They work from any folder and ignore `--dir` and `NOBS_DIR`: `nobs docker network`, `nobs sonda`, `nobs tui`, a `nobs docker` command given `--compose FILE`, and a `nobs containerlab` command given a `TOPOLOGY` file. `nobs workshops` does not need a lab folder either, but it uses `--dir` and `NOBS_DIR` when they are set (see [Workshops](#workshops)).

In the lab folder, `nobs docker` uses `docker-compose.yml`, and with `--scenario NAME` it uses `chapters/NAME/docker-compose.yml`. `nobs containerlab` uses `containerlab/lab.yml` in the book lab. In a workshop or a labcli lab, it uses `containerlab/lab.clab.yaml`, else `containerlab/lab.clab.yml`. Docker Compose groups the containers of one stack under a project name. For a workshop, nobs gives Docker Compose the project name from the workshop's `nobs.toml`.

A labcli lab may also hold a `nobs.toml` with `kind = "lab"`. In it, `compose` and `topology` name other files to use, relative to the lab folder, and `project` sets the project name. nobs also accepts `network`, but does not use it. Each of these is optional.

When nobs finds no lab folder, it prints `nobs: no lab or workshop found from here` and exits with 1. When `--dir` or `NOBS_DIR` names a folder that is none of the three kinds, it prints `nobs: no lab or workshop at PATH` and exits with 1. When nobs cannot read a `nobs.toml` file it meets on the way, it prints the file and what is wrong (the setting, or the line), and exits with 1. In each case nobs runs nothing.

A command that needs a lab folder reads the `.env` file (and the `.setup.env` file, when there is one) of the lab folder. A command that does not need one reads them from the current folder. This is a change from labcli and netobs, which always read the current folder.

## Workshops

A workshop is a lab folder whose `nobs.toml` file has `kind = "workshop"`.

`nobs workshops` lists the workshops nobs can find, one per line: the name, the title and the folder, with two spaces between them, sorted by name. nobs looks in these places:

- the workshop that holds the current folder;
- the workshop that `--dir` or `NOBS_DIR` names;
- each folder directly under the current folder;
- each folder under the `workshops/` folder of the current folder.

| What nobs meets | What it does | Exit code |
|---|---|---|
| no workshop | prints nothing | 0 |
| a `nobs.toml` it cannot read, in a folder below the current folder | skips that folder | 0 |
| a `nobs.toml` it cannot read, upward or in the folder `--dir` or `NOBS_DIR` names | prints the file and what is wrong | 1 |
| `--dir` or `NOBS_DIR` names a folder that is not a lab or workshop | prints `nobs: no lab or workshop at PATH` | 1 |

For example, from a folder that holds `workshops/nobs-t-two/nobs.toml`:

```
$ nobs workshops
nobs-t-two  Workshop two  /home/me/repo/workshops/nobs-t-two
```

When the first word after `nobs` is not one of its commands, nobs takes it as a workshop name. It finds the workshop in the same places as `nobs workshops`, reads the `.env` file of the workshop folder, and runs the workshop command. When it finds no workshop with that name and you are inside a workshop, it takes the word as a command of that workshop instead.

So inside a workshop folder, or below it, you can leave the workshop name out. For example, from `/home/me/repo/workshops/nobs-t-one/sub`, `nobs up` does the same as `nobs nobs-t-one up`. This also works when `--dir` or `NOBS_DIR` names the workshop folder.

The commands of nobs itself (`docker`, `containerlab`, `sonda`, `tui`, `workshops`, `setup`, `preflight` and `help`) keep their meaning in every folder. So inside a workshop, `nobs setup` is still the setup of every workshop (see [Setup and preflight](#setup-and-preflight)), not `nobs <workshop> setup`.

When the first word is not a workshop name, and you are not inside a workshop, nobs prints the error below and exits with 2. The second line lists the names nobs did find, and is left out when there are none.

```
nobs: no workshop named nobs-t-none
known: nobs-t-two
```

`nobs <workshop> --help` lists the commands of a workshop. There are twenty-five, the full set of the workshop's own commands. Sixteen of them make no Docker call, and the second column below says so. Each of the others runs `docker compose --project-name <project> -f <folder>/docker-compose.yml`, then the words in the second column below. `<project>` is the `project` setting in the workshop's `nobs.toml`, and `<folder>` is the workshop folder. `SERVICES` are service names from the compose file; when you give none, the command acts on all of them.

| Command | What it runs | Notes |
|---|---|---|
| `nobs <workshop> setup` | no Docker call | creates `.env` when it is missing |
| `nobs <workshop> up [--build] [SERVICES]` | `up -d [--build] [SERVICES]` | starts in the background; creates `.env` when it is missing; prints the seven addresses after it succeeds; `--build` rebuilds the images built from the workshop's files |
| `nobs <workshop> down` | `down` | removes the containers; the volumes stay |
| `nobs <workshop> destroy` | `down --volumes --remove-orphans` | removes the containers and the volumes |
| `nobs <workshop> restart [SERVICES]` | `restart SERVICES`; with no services, `down`, then `up -d --build` | with no services, prints the seven addresses after it succeeds |
| `nobs <workshop> ps [SERVICES]` | `ps --all [SERVICES]` | lists the containers, stopped ones included |
| `nobs <workshop> logs [--tail N] [--no-follow] [SERVICE]` | `logs -f --tail=N [SERVICE]` | shows the last 200 lines of each service by default and keeps following; `--no-follow` shows the lines and stops, and leaves out `-f` |
| `nobs <workshop> exec SERVICE [COMMAND]` | `exec [-T] SERVICE COMMAND` | runs `sh` when you give no command; adds `-T` when the input is not a terminal; put words that start with a dash after `--` |
| `nobs <workshop> build [SERVICES]` | `build [SERVICES]`, then `up -d [SERVICES]` | builds the images, then starts the services |
| `nobs <workshop> status [OPTIONS]` | no Docker call | asks the seven services if they are up and prints one table; the seven `--…-url` options are in [status](#status) |
| `nobs <workshop> alerts [OPTIONS]` | no Docker call | lists the active alerts of Alertmanager as one table; the options are in [alerts and silences](#alerts-and-silences) |
| `nobs <workshop> silences [OPTIONS]` | no Docker call | lists the silences of Alertmanager as one table; the options are in [alerts and silences](#alerts-and-silences) |
| `nobs <workshop> load-infrahub [OPTIONS]` | no Docker call | applies the schema file to Infrahub, then loads the lab variables into it; the options are in [load-infrahub, schema load and maintenance](#load-infrahub-schema-load-and-maintenance) |
| `nobs <workshop> schema load PATH [OPTIONS]` | no Docker call | applies one schema file to Infrahub; the options are in [load-infrahub, schema load and maintenance](#load-infrahub-schema-load-and-maintenance) |
| `nobs <workshop> maintenance -d NAME [OPTIONS]` | no Docker call | sets or clears the maintenance flag of a device in Infrahub, then tells Sonda; the options are in [load-infrahub, schema load and maintenance](#load-infrahub-schema-load-and-maintenance) |
| `nobs <workshop> rca [DEVICE] [PEER] [OPTIONS]` | no Docker call | prints the most recent AI RCA records from Loki; the options are in [rca and evidence](#rca-and-evidence) |
| `nobs <workshop> evidence DEVICE PEER [OPTIONS]` | no Docker call | prints what Infrahub, Prometheus and Loki say about one BGP session, then a policy hint; the options are in [rca and evidence](#rca-and-evidence) |
| `nobs <workshop> cycle DEVICE PEER [OPTIONS]` | no Docker call | prints four panels about one BGP session: its alert, its silences, the flow runs of the automation and the last decision; with `--trigger` it first starts a new cycle through the webhook; the options are in [cycle and page](#cycle-and-page) |
| `nobs <workshop> page [OPTIONS]` | no Docker call | prints a fake on-call page for a firing alert; the options are in [cycle and page](#cycle-and-page) |
| `nobs <workshop> try-it [OPTIONS]` | no Docker call | walks the four paths of the workshop's alert automation one after another and proves each with a Loki query; the options are in [try-it](#try-it) |
| `nobs <workshop> flap-interface [OPTIONS]` | no Docker call | posts one cascade for an interface and one for each healthy BGP peer on its link to `sonda-server`, so the interface and then BGP go down for a while; the options are in [flap-interface, incident and scenarios](#flap-interface-incident-and-scenarios) |
| `nobs <workshop> incident [OPTIONS]` | no Docker call | posts one `link-failover` cascade to `sonda-server`: a link flaps, the backup link fills up and its latency grows; the options are in [flap-interface, incident and scenarios](#flap-interface-incident-and-scenarios) |
| `nobs <workshop> scenarios [OPTIONS]` | no Docker call | lists the scenarios registered with `sonda-server` as one table; the options are in [flap-interface, incident and scenarios](#flap-interface-incident-and-scenarios) |
| `nobs <workshop> reset [OPTIONS]` | `up sonda-setup`; when a baseline scenario is stuck, `restart sonda-server` and then `up sonda-setup` again; when Loki has no recent log line of `srl1`, `restart sonda-logs` | returns the lab to the state every part of the workshop starts from; it also calls Infrahub, Alertmanager, Sonda and Loki, and goes on after a step that fails; the options are in [reset](#reset) |
| `nobs <workshop> preflight [OPTIONS]` | no Docker call | runs the workshop's own check of the running stack: layer A waits for the data of the stack and makes events on purpose, layer B asks Grafana for every panel of every dashboard; writes three files; leaves `srl1` in maintenance, so run `reset` after it; layer C is not built; the options are in [preflight](#preflight) |

The rules in this paragraph hold for every command except `reset`, which goes on after a step that fails; its warnings and exit codes are in [reset](#reset). When a command makes two calls, nobs stops at the first one that fails. A failed `docker` call makes nobs exit with the code of that call, and `up` and `restart` print no addresses then. When `docker` cannot start, nobs prints `nobs: cannot run docker: <error>` and exits with 1. When the compose file does not exist, nobs prints `nobs: compose file not found: <path>` and exits with 1.

`setup` and `up` create the `.env` file of the workshop from `.env.example` when `.env` is missing, and print `created <folder>/.env from .env.example`. They never change a `.env` that exists: then `setup` prints `kept <folder>/.env` and `up` says nothing. When there is neither a `.env` nor a `.env.example`, both print `no .env.example in <folder>`, and `up` goes on. When the copy fails, both print `nobs: <folder>/.env: <error>` and exit with 1.

The workshops use the same container names and ports, so only one can run at a time. Before `up`, `build` and `restart`, nobs asks Docker for the containers of every other workshop it can find, stopped ones included. When there are some, it prints `nobs: containers of the <title> stack are present and share names and ports with this one: <names>; run nobs <name> down first`, exits with 1 and starts nothing. `<title>` and `<name>` are the title and the name of that other workshop.

After `up`, and after a `restart` with no services, nobs prints the addresses of the seven services.

| Service | Address |
|---|---|
| Grafana | `http://localhost:3000` |
| Prometheus | `http://localhost:9090` |
| Alertmanager | `http://localhost:9093` |
| Loki | `http://localhost:3001` |
| Infrahub | `http://localhost:8000` |
| Prefect | `http://localhost:4200` |
| Sonda | `http://localhost:8085` |

Each line is the service name, padded to 14 characters, and then the address:

```
Grafana       http://localhost:3000
Prometheus    http://localhost:9090
Alertmanager  http://localhost:9093
Loki          http://localhost:3001
Infrahub      http://localhost:8000
Prefect       http://localhost:4200
Sonda         http://localhost:8085
```

A workshop's `nobs.toml` holds `kind = "workshop"` and 11 settings, for example:

```toml
kind = "workshop"
name = "nobs-t-one"
title = "Workshop one"
project = "nobs-t-one"
network = "workshop-nobs-t-one"
workflow_label = "nobs_t_one_quarantine_bgp"
cascade_prefix = "nobs-t-one-cascade-"
scenario_prefix = "nobs-t-one-"
evidence_query = "{device=\"srl1\"}"
silence_hints = ["Workshop", "workshop"]
schema_file = "infrahub/schema.yml"

[image_pins]
sonda = "ghcr.io/davidban77/sonda:1.22.2"
```

Every setting is required except `[image_pins]`, and an unknown key is an error. nobs uses only `name`, `title`, `project`, `schema_file`, `evidence_query`, `workflow_label`, `cascade_prefix` and `silence_hints`; it checks the others but does not use them. `flap-interface` uses `cascade_prefix` as the start of each scenario name, and `title` as the first words of each description (see [flap-interface, incident and scenarios](#flap-interface-incident-and-scenarios)). `reset` uses `silence_hints` to find the silences to expire, and `cascade_prefix` to find the baselines that a cascade left stuck (see [reset](#reset)). `cycle` uses `workflow_label` as the value of the `workflow` label it looks for in Loki (see [cycle and page](#cycle-and-page)).

The `evidence_query` setting is a Loki query. It may hold the placeholders `{device}` and `{peer}`. `nobs <workshop> evidence` replaces each one with the device and the peer you give it, and sends the result to Loki. For example, `evidence_query = "{device=\"{device}\"} != \"license\""` becomes `{device="srl1"} != "license"` for the device `srl1`. The braces around the placeholders belong to the query and stay.

### status

`nobs <workshop> status` asks each of the seven services whether it is up, one after another, and prints one table. It makes no Docker call. Inside a workshop folder, `nobs status` does the same. Run it after `up` to see when the stack is ready.

For each service, nobs sends one HTTP GET request (a request that only reads) to the address plus the path in the table below.

| Service | Path | Flag | Variable | Default address |
|---|---|---|---|---|
| Grafana | `/api/health` | `--grafana-url` | none | `http://localhost:3000` |
| Prometheus | `/-/ready` | `--prom-url` | `PROMETHEUS_URL` | `http://localhost:9090` |
| Loki | `/ready` | `--loki-url` | `LOKI_URL` | `http://localhost:3001` |
| Alertmanager | `/-/ready` | `--am-url` | `ALERTMANAGER_URL` | `http://localhost:9093` |
| Infrahub | `/health` | `--infrahub-url` | `INFRAHUB_ADDRESS` | `http://localhost:8000` |
| Prefect | `/api/health` | `--prefect-url` | none | `http://localhost:4200` |
| Sonda | `/health` | `--sonda-url` | none | `http://localhost:8085` |

For each address, nobs looks at the flag, then the variable when there is one, then the default address. A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. So an empty shell variable does not hide the value in `.env`. A trailing `/` on an address is dropped. When the Infrahub address names `infrahub-server`, the name the containers use among themselves, nobs probes `http://localhost:8000` instead.

nobs waits at most 3 seconds for each service. The table has one header line and one line per service, in the order of the table above, with the columns `Service`, `URL` and `State`. The `URL` column shows the address nobs used. `State` shows one of three things:

| State | When |
|---|---|
| `ok` | the service answered with a status code below 400 |
| `http <code>` | the service answered with a status code of 400 or more, for example `http 503` |
| `down (<reason>)` | the service did not answer, for example because nothing listens on the port or the 3 seconds ran out |

The status code is the number at the start of every HTTP reply. `<reason>` is the error text, and it differs between systems. For example, on macOS a port where nothing listens gives `down (io: Connection refused (os error 61))`.

When every service is `ok`, nobs prints nothing more and exits with 0. Otherwise it prints one more line and exits with 1. That line names the workshop (the `name` setting in its `nobs.toml`), so you can copy the `logs` command from it. In this example the workshop is `nobs-t-one`, and Loki is not ready yet:

```
$ nobs nobs-t-one status
Service       URL                    State
Grafana       http://localhost:3000  ok
Prometheus    http://localhost:9090  ok
Loki          http://localhost:3001  http 503
Alertmanager  http://localhost:9093  ok
Infrahub      http://localhost:8000  ok
Prefect       http://localhost:4200  ok
Sonda         http://localhost:8085  ok
Some services are not ready yet: give them a minute, or check nobs nobs-t-one logs.
```

### alerts and silences

`nobs <workshop> alerts` lists the active alerts of Alertmanager as one table. `nobs <workshop> silences` lists its silences as one table. Each alert carries labels: pairs of a key and a value, like `device=srl1`. A silence is a rule in Alertmanager that mutes the alerts whose labels match it, for a set time. An alert is inhibited when an inhibit rule in Alertmanager mutes it because another, related alert is firing. Neither command makes a Docker call. Each sends one HTTP GET request (a request that only reads) to Alertmanager and waits at most 10 seconds for the answer. Inside a workshop folder, `nobs alerts` and `nobs silences` do the same.

| Command | Flag | What it does |
|---|---|---|
| both | `--am-url URL` | gives the address of Alertmanager |
| `alerts` | `-l KEY=VALUE`, `--label KEY=VALUE` | shows only the alerts that have this label; repeat it for more labels, and an alert must have them all |
| `alerts` | `--silenced`, `--no-silenced` | shows or leaves out the silenced alerts; they are shown by default |
| `alerts` | `--inhibited`, `--no-inhibited` | shows or leaves out the inhibited alerts; they are shown by default |
| `silences` | `--show-expired` | shows the expired silences too |

When you give both flags of a pair, the last one wins. For the address, nobs looks at `--am-url`, then the variable `ALERTMANAGER_URL`, then the default address `http://localhost:9093`. A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. A trailing `/` on an address is dropped.

For `alerts`, nobs asks Alertmanager for the active alerts, and tells it to leave out the silenced or inhibited ones when you give `--no-silenced` or `--no-inhibited`. nobs splits each label at the first `=` and trims spaces around the key and the value. When the same key is given twice, the last value counts. A label without `=` is a mistake: nobs checks every label before it sends anything, and when one is wrong it prints `nobs: --label must be key=value (got '<text>')` and exits with 2.

The table has one header line and one line per alert, in the order Alertmanager sent them. For example, with two alerts:

```
$ nobs nobs-t-one alerts
Alertname        Severity  Device / target      State     Age
BgpSessionNotUp  critical  srl1 → 10.0.0.2      firing    20s
InterfaceDown    warning   srl2 → ethernet-1/1  silenced  20s
```

| Column | What it shows |
|---|---|
| `Alertname` | the `alertname` label, or `—` when the alert has none |
| `Severity` | the `severity` label, or `—` when it is missing or empty |
| `Device / target` | the `device` label, or `—` when it is missing. Then an arrow and the target, when the alert has one, as in `srl1 → 10.0.0.2`. The target is the `peer_address` label, or else the `interface` label. An empty label counts as missing. |
| `State` | what the alert is now; see the next table |
| `Age` | the time since the alert started: `42s`, `3m05s`, `2h07m` or `1d3h`; `—` when nobs cannot read the start time |

`State` shows the first row of this table that fits the alert:

| State | When |
|---|---|
| `silenced` | a silence covers the alert |
| `inhibited` | another alert inhibits it |
| `firing` | Alertmanager sent the state `active` |
| the state Alertmanager sent, in lower case | Alertmanager sent another state |
| `—` | Alertmanager sent no state |

When no alert is left to show, nobs prints the header line, then `No alerts match.`, and exits with 0.

`silences` leaves out the expired silences unless you give `--show-expired`. The table has one header line and one line per silence, in the order Alertmanager sent them. For example, with `--show-expired`:

```
$ nobs nobs-t-one silences --show-expired
ID             State    Starts (UTC)  Ends (UTC)  Remaining  Matchers                                Creator
0a1b2c3d-4e5…  active   00:50:09      02:50:09    60m0s      alertname=BgpSessionNotUp, device=srl1  alice-the-very
ffffffff-000…  expired  00:50:09      01:49:09    —          device=srl2                             bob
```

| Column | What it shows |
|---|---|
| `ID` | the first 12 characters of the silence id, then `…` |
| `State` | the state Alertmanager sent, for example `active`, `pending` or `expired`, or `?` when it sent none |
| `Starts (UTC)` | the time part `HH:MM:SS` of the start, in UTC; empty when the time is missing |
| `Ends (UTC)` | the time part `HH:MM:SS` of the end, in UTC; empty when the time is missing |
| `Remaining` | the time until the end as `<minutes>m<seconds>s`, for example `60m0s` (minutes are not turned into hours); `—` when the end has passed, and `?` when nobs cannot read the end time |
| `Matchers` | `name=value` for each matcher, joined by `, `; nobs does not show whether a matcher is a regular expression or a "not equal" matcher |
| `Creator` | the first 14 characters of the name of the creator |

When no silence is left to show, nobs prints one line and no table, and exits with 0. `<address>` is the address nobs used:

| When | Line |
|---|---|
| without `--show-expired` | `No silences active or pending in Alertmanager at <address>.` |
| with `--show-expired` | `No silences (any state) in Alertmanager at <address>.` |

| Exit code | When |
|---|---|
| 0 | nobs printed the table, or the line that says there is nothing to show |
| 1 | Alertmanager did not answer: `nobs: cannot reach Alertmanager at <address>: <error>` |
| 1 | Alertmanager answered with a status code of 400 or more: `nobs: Alertmanager at <address> answered <status>` |
| 1 | the answer is not a JSON list: `nobs: Alertmanager at <address> answered with a body nobs cannot read: <reason>` |
| 2 | a `--label` without `=`, or a flag nobs does not know |

### load-infrahub, schema load and maintenance

Infrahub is the database where the workshop keeps its devices, interfaces and BGP sessions. Each device, interface or BGP session in Infrahub is a node, and each node has a kind, for example `WorkshopDevice`. A schema file tells Infrahub which kinds of node it holds. A lab variables file lists the nodes to put in. Three commands talk to Infrahub, and none makes a Docker call. nobs sends their requests to Infrahub itself, over plain HTTP: an `http://` address, with no encryption.

| Command | What it does |
|---|---|
| `nobs <workshop> load-infrahub` | applies the schema file, then loads the lab variables file: it creates the nodes that are missing and updates the ones that exist, so you can run it again |
| `nobs <workshop> schema load PATH` | applies one schema file |
| `nobs <workshop> maintenance -d NAME` | sets or clears the `maintenance` flag of one device in Infrahub, then tells Sonda |

| Command | Flag or argument | What it does |
|---|---|---|
| `load-infrahub` | `--schema PATH` | gives the schema file; the default is the `schema_file` setting of `nobs.toml` |
| `load-infrahub` | `--lab-vars PATH` | gives the lab variables file; the default is `lab_vars.yml` |
| `load-infrahub` | `--skip-schema` | does not apply the schema, and does not look for the schema file |
| `schema load` | `PATH` | gives the schema file |
| `maintenance` | `-d NAME`, `--device NAME` | gives the name of the device; this flag is required |
| `maintenance` | `--state`, `--clear` | sets the device's `maintenance` flag to true or to false; the last one wins, and `--state` is the default |
| `maintenance` | `--kind KIND` | gives the kind of the node; the default is `WorkshopDevice` |
| all three | `--address URL` | gives the address of Infrahub |
| all three | `--token TOKEN` | gives the token: the secret key that lets nobs use Infrahub |

A relative path is taken from the workshop folder, not from the current folder. An absolute path is used as it is.

nobs finds the address and the token in this order: the flag, then the variable, then the default.

| Value | Flag | Variable | Default |
|---|---|---|---|
| the address of Infrahub | `--address URL` | `INFRAHUB_ADDRESS` | `http://localhost:8000` |
| the token | `--token TOKEN` | `INFRAHUB_API_TOKEN` | none |

A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. A token you give with `--token` is on the command line of nobs itself, and a list of processes shows it. So keep the token in the `.env` file.

The containers reach Infrahub by the name `infrahub-server`, and your machine cannot. When the address names `infrahub-server`, nobs uses `http://localhost:8000` instead. Before any other line, it then prints the rewrite line: `INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000`. The line says `INFRAHUB_ADDRESS` even when the address came from `--address`. A trailing `/` is dropped without a message.

`load-infrahub` and `maintenance` check the token first. Then `load-infrahub` reads the schema file (unless you give `--skip-schema`) and the lab variables file. `schema load` first checks that its schema file is there, then checks the token, then reads the file. `load-infrahub` and `schema load` send nothing to Infrahub before they have read their files. When a file is missing, nobs prints `nobs: schema file not found: <path>` or `nobs: lab_vars file not found: <path>`. When a file is there but is not valid YAML, or does not have the shape nobs expects, nobs prints `nobs: cannot read <path>: <reason>`.

`load-infrahub` prints the rewrite line when there is one, then `Applying schema <path>` and `Schema applied` (both left out with `--skip-schema`), then `Loading <path> into Infrahub at <address>`. Infrahub needs a moment before it shows the kinds of a new schema, so nobs waits up to 30 seconds until Infrahub shows `WorkshopDevice`, `WorkshopInterface` and `WorkshopBgpSession`. Then nobs prints one progress line for each device, as soon as it has created or updated that device, for example `device srl1 created` or `device srl1 updated`. After that line, nobs loads the interfaces and BGP sessions of the device, and prints no line for them. Then one table and one last line. For example:

```
$ nobs nobs-t-one load-infrahub
INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000
Applying schema /home/me/repo/workshops/nobs-t-one/infrahub/schema.yml
Schema applied
Loading /home/me/repo/workshops/nobs-t-one/lab_vars.yml into Infrahub at http://localhost:8000
device srl1 created
device srl2 created
Node type           Created  Updated  Total
WorkshopDevice      2        0        2
WorkshopInterface   8        0        8
WorkshopBgpSession  6        0        6
Done: 16 nodes across 3 types. Open Infrahub at http://localhost:8000.
```

The table has one line for each kind of node, and `Total` is `Created` plus `Updated`. In the last line, the number is the sum of the `Total` column. Run the command again, and the same nodes show under `Updated` instead of `Created`. When the lab variables file lists no device, nobs also prints `nobs: no devices found in <path>: is --lab-vars the right file?` after the last line, on standard error, the output a program uses for errors and warnings. The exit code stays 0.

`schema load` prints the rewrite line when there is one, then `Applying schema <path>` and `Schema applied`.

The `maintenance` flag is a field of each device in Infrahub, `true` or `false`. The `maintenance` command prints the rewrite line when there is one. When Infrahub has no node of that kind with that name, nobs then prints `nobs: <kind> <device> not found in Infrahub.`, exits with 1 and tells Sonda nothing. Otherwise nobs saves the flag and prints `<device>.maintenance: <before> → <after>`, where each value is `false` or `true`. Then it posts one event to Sonda, and Sonda sends it on to Loki. Grafana shows the event on its annotation panel; an annotation is a mark on a graph at the time of an event. The event is a log line with these parts:

| Part | Value |
|---|---|
| label `device` | the name of the device |
| label `source` | `workshop-trigger` |
| label `event` | `config-push` |
| label `level` | `info` |
| message | `Configured from CLI: <device>.maintenance = True`, or `= False` with `--clear` |

Last, nobs prints one of these lines:

| Flag | Line |
|---|---|
| `--state` | `The next alert for this device will be SKIPPED by the policy.` |
| `--clear` | `The next alert for this device will be evaluated normally.` |

```
$ nobs nobs-t-one maintenance -d srl1 --state
INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000
srl1.maintenance: false → true
The next alert for this device will be SKIPPED by the policy.
```

| Variable | What it gives | Default |
|---|---|---|
| `SONDA_SERVER_URL` | the address of Sonda | `http://localhost:8085` |
| `SONDA_API_KEY` | the key nobs sends to Sonda | none |
| `SONDA_LOKI_SINK_URL` | the address of Loki as Sonda sees it, where Sonda sends the event | `http://loki:3001` |

These three variables follow the same rule as the Infrahub variables: the shell or the `.env` file, the shell wins, and a value that is empty or only spaces counts as not given. The default address of Sonda here is `http://localhost:8085`, the address of the workshop's Sonda; `nobs sonda` uses `http://localhost:8080` by default. nobs waits at most 35 seconds for Sonda.

| Exit code | When |
|---|---|
| 0 | the command finished |
| 1 | there is no token: `nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).` |
| 1 | a file is missing: `nobs: schema file not found: <path>` or `nobs: lab_vars file not found: <path>` |
| 1 | a file cannot be read: `nobs: cannot read <path>: <reason>` |
| 1 | Infrahub did not answer: `nobs: cannot reach <address>: <reason>` |
| 1 | Infrahub answered with a status code other than 200: `nobs: Infrahub answered <status> on <path>: <answer>`, where `<answer>` is the first 200 characters of the answer |
| 1 | Infrahub answered 200 with an error message: `nobs: Infrahub: <message>` |
| 1 | Infrahub did not show all three kinds of node in 30 seconds: `nobs: workshop schema kinds <kinds> not visible to Infrahub after 30 s`, where `<kinds>` are the ones still missing |
| 1 | `maintenance` found no such node: `nobs: <kind> <device> not found in Infrahub.` |
| 1 | the post to Sonda failed: `nobs: sonda /events post failed: <error>`; the flag stays saved in Infrahub |
| 2 | a flag nobs does not know, a missing `-d`, or a missing `PATH` after `schema load` |

In the post failure line, `<error>` is `cannot reach <address>: <reason>` when Sonda does not answer, or `sonda answered <status>: <body>` when it answers with a status code of 400 or more.

The files under `captures/` are the record of what the Python SDK (Infrahub's own client library) sent to Infrahub 1.1 and 1.11.2, and what Infrahub answered. The tests of nobs use them.

### rca and evidence

Two commands show what the running stack knows about a BGP session. Both only read, and neither makes a Docker call. Inside a workshop folder, `nobs rca` and `nobs evidence` do the same. RCA means root cause analysis. An AI RCA record is a log line in Loki, written by the workshop's automation, that holds a short explanation of why an alert fired. SoT means source of truth: what Infrahub holds about the lab.

| Command | What it does |
|---|---|
| `nobs <workshop> rca [DEVICE] [PEER]` | prints the most recent AI RCA records from Loki |
| `nobs <workshop> evidence DEVICE PEER` | prints what Infrahub, Prometheus and Loki say about one BGP session of one device, then a policy hint |

| Command | Argument or flag | What it does |
|---|---|---|
| `rca` | `DEVICE` | shows only the records of this device; optional |
| `rca` | `PEER` | shows only the records of this peer address; optional; to give a peer without a device, give an empty device: `rca "" 10.1.99.2` |
| `rca` | `--last N` | gives how many of the most recent records to show; the default is 1 |
| `rca` | `--minutes N` | gives how far back to look; the default is 60 |
| `rca` | `--loki-url URL` | gives the address of Loki |
| `evidence` | `DEVICE`, `PEER` | give the device and the IP address of its peer; both are required |
| `evidence` | `--afi-safi NAME` | gives the address family of the session; the default is `ipv4-unicast` |
| `evidence` | `--instance NAME` | gives the network instance of the session; the default is `default` |
| `evidence` | `--log-minutes N` | gives how many minutes back to look for log lines; the default is 10 |
| `evidence` | `--log-limit N` | gives how many log lines to show at most; the default is 20 |
| `evidence` | `--prom-url URL`, `--loki-url URL`, `--infrahub-url URL` | give the addresses of Prometheus, Loki and Infrahub |
| `evidence` | `--token TOKEN` | gives the token for Infrahub |

nobs finds each address and the token in this order: the flag, then the variable, then the default.

| Value | Flag | Variable | Default |
|---|---|---|---|
| the address of Prometheus (`evidence`) | `--prom-url URL` | `PROMETHEUS_URL` | `http://localhost:9090` |
| the address of Loki | `--loki-url URL` | `LOKI_URL` | `http://localhost:3001` |
| the address of Infrahub (`evidence`) | `--infrahub-url URL` | `INFRAHUB_ADDRESS` | `http://localhost:8000` |
| the token for Infrahub (`evidence`) | `--token TOKEN` | `INFRAHUB_API_TOKEN` | none |

A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. A trailing `/` on an address is dropped. Each call waits at most 10 seconds. `evidence` treats the Infrahub address and the token as `load-infrahub` does (see [load-infrahub, schema load and maintenance](#load-infrahub-schema-load-and-maintenance)): when the address names `infrahub-server`, nobs prints the rewrite line first and uses `http://localhost:8000`, and with no token it prints the required-token line and sends nothing.

`rca` sends one request to Loki for the lines that carry the labels `source="prefect"` and `ai_rca="true"`, and the labels `device` and `peer_address` when you give them. It asks for the newest lines first, at most `--last` of them. When Loki returns no line, nobs prints one line, where `<scope>` is `device=D peer=P`, `device=D`, `peer=P` or `any device/peer`, `<folder>` is the workshop folder, and `<name>` is the `name` setting in its `nobs.toml`:

```
No AI RCA records found in the last <minutes>m for <scope>. If you haven't enabled it yet, set ENABLE_AI_RCA=true in <folder>/.env and re-run `nobs <name> up`.
```

Otherwise nobs prints a blank line, then one entry for each line, newest first. A line that is not JSON gives `(skipping unparseable record #<n>)`, where `<n>` is the place of that line in the list, counting from 1. A JSON record gives a first line `<timestamp>  device=<device>  peer=<peer>`, from the record's `timestamp` and its `device` and `peer_address` labels. A missing timestamp leaves that part empty, and a missing label shows as `?`. Then come the record's `message` as plain text and a blank line. nobs removes a leading `AI RCA:` from the message, and the line breaks right after it; a space after it stays. It does not render Markdown. For example:

```
$ nobs nobs-t-one rca srl1 10.1.99.2 --last 2

2026-11-06T10:15:48.201Z  device=srl1  peer=10.1.99.2
## Most likely cause
The peer 10.1.99.2 stopped answering.

(skipping unparseable record #2)
```

`evidence` asks Infrahub for the device, asks Prometheus for six numbers one after another, and asks Loki for log lines once. A query to Prometheus or Loki that fails, or that finds nothing, never changes the exit code: the number counts as missing, and the log lines as none. The six numbers are `bgp_admin_state`, `bgp_oper_state`, `bgp_received_routes`, `bgp_sent_routes`, `bgp_active_routes` and `bgp_suppressed_routes`, each with the labels `device`, `peer_address`, `afi_safi_name` and `name` (the instance). The Loki query is the `evidence_query` setting of the workshop with `{device}` and `{peer}` filled in (see [Workshops](#workshops)); `--log-minutes` and `--log-limit` set its window and its size.

Then nobs prints a blank line and four blocks, separated by blank lines. Each block is a title line and its body. For example:

```
$ nobs nobs-t-one evidence srl1 10.1.99.2

Source of truth (Infrahub)
device          srl1   site=lab  role=edge
maintenance     false
intended peer   yes
expected state  established
reason          ip-mismatch-demo
remote_as       65102

BGP metrics snapshot (Prometheus)
Metric             Value  Decoded
admin_state            1  enable
oper_state             5  active
received_routes        0  —
sent_routes           10  —
active_routes         10  —
suppressed_routes      0  —

Loki - last 2 relevant line(s)
2026-11-06T10:20:00Z srl1 bgp neighbor 10.1.99.2 session state changed to active
2026-11-06T10:15:48Z srl1 bgp neighbor 10.1.99.2 session state changed to idle

Policy hint
decision: proceed
reason  : SoT expects peer up, but metrics show mismatch
```

The first block depends on what Infrahub answered. A peer is intended when the device has a BGP session with that peer address and with the `--afi-safi` family (a session with no family counts as `ipv4-unicast`). The lines `expected state`, `reason` and `remote_as` come from that session, and only when the peer is intended. `expected state` is `—` when the session has none, `reason` is left out when it is missing or empty, and `remote_as` is left out when it is missing or 0. A missing `site` or `role` shows as `?`.

| Infrahub | First block |
|---|---|
| has the device | `Source of truth (Infrahub)` and the lines of the example |
| has no such device | `Source of truth`, then `device not found in Infrahub.` |
| did not answer, or answered with an error | `Source of truth`, then `Infrahub query failed: <error>`, where `<error>` is `cannot reach <address>: <reason>`, `Infrahub answered <status> on <path>: <answer>` or `Infrahub: <message>` |

In the second block, the `Value` column is right-aligned. A whole number shows without a fraction (`1`, `10`) and any other number as it is (`0.5`). A missing `admin_state` or `oper_state` shows as `-1`, a missing counter as `0`, and a number that is not finite as `?`. `Decoded` is `enable` for an `admin_state` of 1 and `disable` for 2, and `up`, `down`, `idle`, `connect` or `active` for an `oper_state` of 1 to 5. Any other number is `unknown`, a missing state is `—`, and the four counters always show `—`.

The third block is `Loki - last <n> relevant line(s)` with the lines as Loki holds them, newest first. When there is none, the block is `Loki` and `No relevant log lines in the window.`

The last block shows the first row of this table that fits:

| decision | reason | When |
|---|---|---|
| `stop` | `device not found in Infrahub`, or the `Infrahub query failed` text | the first block is not `Source of truth (Infrahub)` |
| `skip` | `device under maintenance` | the device has `maintenance` set to true |
| `skip` | `peer not intended in SoT` | the peer is not intended |
| `skip` | `SoT expects this peer to be down/disabled` | the expected state, in lower case, is `down` or `disabled`; with no expected state, nobs reads `established` |
| `skip` | `peer matches SoT intent (enabled + up)` | `admin_state` is 1 and `oper_state` is 1 |
| `proceed` | `SoT expects peer up, but metrics show mismatch` | none of the rows above fits |

| Exit code | When |
|---|---|
| 0 | `rca` printed records or the line that says there are none; `evidence` printed its four blocks |
| 1 | `rca` could not get an answer from Loki: `nobs: Failed to query Loki at <address>: <reason>`. `<reason>` is one of three texts: why nobs could not connect; `Loki answered <status> on /loki/api/v1/query_range: <answer>`, where `<answer>` is the first 200 characters of the answer; or `Loki answered with text nobs cannot read: <error>`, where `<error>` says what was wrong with the answer |
| 1 | `evidence` has no token: `nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).` |
| 2 | a flag nobs does not know, a missing `DEVICE` or `PEER` for `evidence`, or a value that is not a whole number of 0 or more for `--last`, `--minutes`, `--log-minutes` or `--log-limit` |

### cycle and page

`cycle` shows one BGP session from the side of the automation that reacts to its alert. `page` shows a firing alert as an on-call page: the short message that wakes the person on duty. Both only read, except `cycle --trigger`, and neither makes a Docker call. Inside a workshop folder, `nobs cycle` and `nobs page` do the same. Prefect is the tool that runs the automation flows of the workshop, and a flow run is one run of a flow. The webhook is a web address that receives an alert and starts the flow that handles it. An annotation is a pair of a key and a value, like a label, but it holds text for people, such as `summary`.

| Command | What it does |
|---|---|
| `nobs <workshop> cycle DEVICE PEER` | prints four panels about one BGP session: its alert, its silences, the flow runs of the automation and the last decision |
| `nobs <workshop> page` | prints a fake on-call page for a firing alert, as if a pager had just gone off |

| Command | Argument or flag | What it does |
|---|---|---|
| `cycle` | `DEVICE`, `PEER` | give the device and the IP address of its peer; both are required |
| `cycle` | `--trigger` | before the panels, sends a new alert to the webhook and waits for the flow run it starts |
| `cycle` | `--status STATUS` | gives the status of the alert that `--trigger` sends, `firing` or `resolved`; the default is `firing`; any other value is a usage error, with or without `--trigger` |
| `cycle` | `--minutes N` | gives how far back the flow runs and the decision are looked for; the default is 30 |
| `cycle` | `--am-url URL`, `--loki-url URL`, `--prefect-url URL`, `--webhook-url URL` | give the addresses of Alertmanager, Loki, Prefect and the webhook |
| `page` | `-a ALERT`, `--alert ALERT` | shows only an alert with this name; the default is `BgpSessionNotUp`, and an empty name filters nothing |
| `page` | `-d DEVICE`, `--device DEVICE` | shows only an alert on this device; the default is empty, which filters nothing |
| `page` | `-p PEER`, `--peer PEER` | shows only an alert for this peer address; the default is empty, which filters nothing |
| `page` | `--now HH:MM` | gives the time printed after `PAGED`; the default is the current time in UTC |
| `page` | `--am-url URL` | gives the address of Alertmanager |

nobs finds each address in this order: the flag, then the variable, then the default.

| Value | Flag | Variable | Default |
|---|---|---|---|
| the address of Alertmanager | `--am-url URL` | `ALERTMANAGER_URL` | `http://localhost:9093` |
| the address of Loki (`cycle`) | `--loki-url URL` | `LOKI_URL` | `http://localhost:3001` |
| the address of Prefect (`cycle`) | `--prefect-url URL` | `PREFECT_API_URL` | `http://localhost:4200` |
| the address of the webhook (`cycle`) | `--webhook-url URL` | `WEBHOOK_URL` | `http://localhost:9997/v1/api/webhook` |

A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. A trailing `/` on the address of Alertmanager, Loki or Prefect is dropped, and so is a trailing `/api` on the address of Prefect, so `http://localhost:4200/api` works as well as `http://localhost:4200`. The address of the webhook is used as it is given. `nobs <workshop> status` does not read `PREFECT_API_URL`. Each call waits at most 10 seconds, and the post to the webhook waits at most 15.

`cycle` prints a blank line, a title line and a body for each of four panels, in the order of the table below. When a call fails, the panel keeps its title and its body is the error. The exit code stays 0 whatever the panels found. For example, with an alert that a silence mutes, and a flow that started three child flows:

```
$ nobs nobs-t-one cycle srl1 10.1.99.2

Alert
BgpSessionNotUp · warning · state=suppressed · age 4m29s
silencedBy: 3f2a9c1e-7b4…

Silences (this peer)
ID             State   Starts    Ends      Remaining
3f2a9c1e-7b4…  active  10:16:00  10:36:00  16m0s

Prefect flow runs (last 30m)
Started   State      Flow
10:15:40  COMPLETED  quarantine_bgp | srl1:10.1.99.2
10:15:41  COMPLETED    ├─ evidence
10:15:44  COMPLETED    ├─ policy
10:15:45  COMPLETED    └─ action

Most recent decision
10:15:48  decision=proceed
reason: SoT expects peer up, but metrics show mismatch

  To drive a fresh cycle (bypassing Alertmanager's repeat_interval):
    nobs nobs-t-one cycle srl1 10.1.99.2 --trigger
```

| Panel | What it shows | When nothing is found | When the call fails |
|---|---|---|---|
| `Alert` | the alerts named `BgpSessionNotUp` whose `device` and `peer_address` labels are `DEVICE` and `PEER`, from one call to Alertmanager that includes silenced and inhibited alerts | `No BgpSessionNotUp alert active for <device> → <peer>.` | `Alertmanager query failed: <error>` |
| `Silences (this peer)` | the silences that match `device` equal to `DEVICE` and `peer_address` equal to `PEER`, in every state, the newest start first, at most 5 | `No silences scoped to <device> → <peer>.` | `silences query failed: <error>` |
| `Prefect flow runs (last <minutes>m)` | the flow runs of this device and peer that started within `--minutes`, the newest first, at most 5 runs, each with its child runs | `No quarantine_bgp / resolved_bgp runs for <device>:<peer> in the last <minutes>m.` | `Prefect query failed: <error>` |
| `Most recent decision` | the newest decision record in Loki for this device and peer within `--minutes` | `No decision audit records for <device> → <peer> in the last <minutes>m.` | `Loki query failed: <error>` |

In the last column, `<error>` is the text of the failed call, and `<address>` is the address nobs used, without a trailing `/` (and, for Prefect, without a trailing `/api`). For Alertmanager it is `cannot reach Alertmanager at <address>: <reason>`, `Alertmanager at <address> answered <status>` or `Alertmanager at <address> answered with a body nobs cannot read: <problem>`. For Prefect it is `cannot reach <address>: <reason>`, `Prefect answered <status> on /api/flow_runs/filter: <answer>` or `Prefect answered with text nobs cannot read: <problem>`. For Loki it is `cannot reach <address>: <reason>`, `Loki answered <status> on /loki/api/v1/query_range: <answer>` or `Loki answered with text nobs cannot read: <problem>`. `<reason>` is why nobs could not connect, `<answer>` is the first 200 characters of the answer, and `<problem>` says what was wrong with the answer. The `page` fetch error uses the Alertmanager texts.

Each alert in the first panel gives two lines. The first is `BgpSessionNotUp · <severity> · state=<state> · age <age>`. `<severity>` is the `severity` label, or `?` when the alert has none. `<state>` is `firing` when Alertmanager sent the state `active`, `pending` for `unprocessed`, `?` when it sent none, and otherwise the state as it sent it, for example `suppressed`. `<age>` is the time since the alert started, written as in the `alerts` table (`42s`, `3m05s`, `2h07m` or `1d3h`), or `?` when nobs cannot read the start time. The second line is `silencedBy: <id>…` with the first 12 characters of the first silence that mutes the alert, followed by ` (+<n> more)` when `<n>` more silences mute it, or `silencedBy: (none)`.

The silence table has one header line and one line per silence, with these columns:

| Column | What it shows |
|---|---|
| `ID` | the first 12 characters of the silence id, then `…`; `?` when it has none |
| `State` | the state Alertmanager sent, for example `active`, `pending` or `expired`, or `?` when it sent none |
| `Starts` | the time part `HH:MM:SS` of the start, in UTC |
| `Ends` | the time part `HH:MM:SS` of the end, in UTC |
| `Remaining` | the time until the end as in `silences`, for example `16m0s`; `—` when the end has passed, and `?` when nobs cannot read the end time |

For the flow runs, nobs asks Prefect for the 100 newest flow runs and keeps those whose `device` and `peer_address` parameters are `DEVICE` and `PEER`. Some flows start other flows: the run that starts another run is its parent, and the run it starts is a child. Prefect links a child to its parent through a task run, one step inside the parent run. When a selected run has a parent, nobs asks Prefect once for those task runs, and shows each child under its parent as `├─ <name>`, with `└─ <name>` for the last one, the oldest first. The name of a child is cut before the first ` | `. A run whose parent is not among the selected runs gets its own line. A run whose parent is itself a child is not shown. When no selected run has a parent, or when the call for the task runs fails, the list is flat. The columns are `Started` (the time part `HH:MM:SS` of the start, in UTC), `State` (the state type that Prefect sent, for example `COMPLETED`, `RUNNING` or `FAILED`, or `?`) and `Flow` (the name of the run, or `?` when it has none).

For the decision, nobs sends one request to Loki for the newest line that has the labels `source="prefect"`, `workflow` equal to the `workflow_label` setting of the workshop, `device`, `peer_address` and a `decision` that is not empty. It prints `<time>  decision=<decision>` and `reason: <message>`, where `<time>` is the `HH:MM:SS` part of the `timestamp` of the line, `<decision>` is its `decision` label (`?` when it is missing) and `<message>` is its `message`. When the line is not a JSON object, the body is `Could not parse latest decision JSON.`

Without `--trigger`, nobs ends with the blank line and the two lines of the example above, where `<name>` is the `name` setting of the workshop. With `--trigger`, nobs first does this:

| Step | What nobs does |
|---|---|
| 1 | asks Prefect for the flow runs of this device and peer that started in the last 120 minutes, and notes the start of the newest one; when there is none, or the call fails, it notes the current time |
| 2 | sends one alert for `BgpSessionNotUp`, with the status from `--status`, to the webhook; this skips the `repeat_interval` of Alertmanager, the time it waits before it sends the same alert again |
| 3 | prints `posted <status> payload for <device> → <peer>; waiting for new flow run …` |
| 4 | asks Prefect again every 2 seconds, at most 15 times and at most 30 seconds in all, for the flow runs of the last 120 minutes; as soon as an answer holds a run that started after the note, it prints `new flow run started at <HH:MM:SS>`, the start of the newest such run; child runs count too, so this can be the start of a child, a few seconds after the new cycle's first run; when no answer holds one it prints `no new flow run appeared within 30s; rendering current state anyway.`; a call that fails counts as no run |
| 5 | waits 2 more seconds, then prints the four panels, without the last two lines of the example above |

`page` asks Alertmanager once for the alerts. It keeps the alerts in the state `active`, and, for each of `-a`, `-d` and `-p` that is not empty, the alerts whose `alertname`, `device` or `peer_address` label is equal to it. An alert that a silence or an inhibit rule mutes has the state `suppressed`, so once the automation has silenced a peer, `page` does not find its alert. nobs prints the page of the first alert that is left. The folder in the last line is the workshop folder as nobs found it. For example:

```
$ nobs nobs-t-one page -d srl1 -p 10.1.99.2 --now 02:14
on-call
PAGED 02:14
BgpSessionNotUp on srl1 — peer 10.1.99.2
BGP peer 10.1.99.2 not reaching Established
firing for: ~4 min
severity: warning

You're awake. The dashboard is your only friend.

  Confirm the alert: nobs nobs-t-one alerts
  Open the runbook:  /home/me/repo/workshops/nobs-t-one/guides/
```

| Line | What it shows |
|---|---|
| `PAGED <time>` | the time you gave with `--now`, as you gave it; without `--now`, or with an empty one, the current time in UTC as `HH:MM`, because nobs does not know the time zone of the machine |
| the third line | the `alertname` label (`unknown alert` when it is missing), then ` on <device>` when the `device` label is set and not empty, then ` — peer <peer_address>` when the `peer_address` label is set and not empty |
| the fourth line | the `summary` annotation of the alert when it has one; otherwise `BGP peer <peer_address> not reaching Established` for `BgpSessionNotUp`, `interface <interface> flapping above threshold` for `PeerInterfaceFlapping`, and `alert <alertname> firing` for any other alert, where a missing label shows as `?` |
| `firing for: <time>` | the time since the alert started, rounded: `<1 min`, `~<m> min` under an hour, `~<h>h<mm>m` from an hour, and `unknown` when nobs cannot read the start time |
| `severity: <severity>` | the `severity` label, or `unknown` when it is missing |
| the last two lines | the command that lists the alerts, with the `name` setting of the workshop, and the `guides/` folder of the workshop |

| Exit code | When |
|---|---|
| 0 | `cycle` printed its panels, whatever they found; `page` printed the page |
| 1 | `cycle --trigger` could not post to the webhook: `nobs: could not POST to webhook: <error>`, where `<error>` is `cannot reach <address>: <reason>` or `the webhook answered <status>: <answer>`, and `<answer>` is the first 200 characters of the answer |
| 1 | `page` could not get the alerts: `nobs: Alertmanager fetch failed: <error>` |
| 1 | `page` found no active alert that fits the filters: ``nobs: no firing alert matched the filters. Run `nobs <name> alerts` to see what's currently firing.`` |
| 2 | a flag nobs does not know, a missing `DEVICE` or `PEER` for `cycle`, a `--status` other than `firing` or `resolved`, or a value that is not a whole number of 0 or more for `--minutes` |

### try-it

`try-it` walks the four paths of the alert automation of the workshop, one after another, and proves each path with a query to Loki. The webhook is the web address that receives an alert and starts the flow that handles it. For each path, nobs sends a fake alert to the webhook, then asks Loki until a log line shows what the automation decided. A Loki match is a log line in Loki that fits the query of the path. The maintenance flag is a setting of a device in Infrahub: while it is true, the automation skips the alerts of that device. `try-it` changes the lab: each alert starts real flow runs, and path 2 sets and then clears the maintenance flag of `srl1`. To run it with nobody at the keyboard, for example in CI (an automatic test job), give `--auto`. Inside a workshop folder, `nobs try-it` does the same. It makes no Docker call.

The four paths always run in this order. Every path uses the device `srl1`. The panel title of a path is `Path <number> - <title>`. `<workflow_label>` is the `workflow_label` setting of the workshop's `nobs.toml`.

| Path | Title | What nobs sends | The Loki query nobs waits for |
|---|---|---|---|
| 1 | Actionable / mismatch → proceed | a `firing` alert for `srl1` and its peer `10.1.99.2` | `{source="prefect", workflow="<workflow_label>", device="srl1", peer_address="10.1.99.2", decision="proceed"}` |
| 2 | In-maintenance → skip | a `firing` alert for `srl1` and its peer `10.1.99.2`; before it, nobs sets the maintenance flag of `srl1` in Infrahub to true, and after the wait it sets it back to false | `{source="prefect", workflow="<workflow_label>", device="srl1", decision="skip"}` |
| 3 | Healthy peer → skip | a `firing` alert for `srl1` and its peer `10.1.2.2` | `{source="prefect", workflow="<workflow_label>", device="srl1", peer_address="10.1.2.2", decision="skip"}` |
| 4 | Resolved → audit | a `resolved` alert for `srl1` and its peer `10.1.99.2` | `{source="prefect", workflow="<workflow_label>", device="srl1", decision="resolved"}` |

Unlike `nobs <workshop> maintenance`, `try-it` does not tell Sonda about the flag, so no maintenance event from Sonda reaches Loki.

One run goes through these steps in order. Steps 4 to 9 repeat for each path.

| Step | What nobs does |
|---|---|
| 1 | reads the addresses and the token (see the table below), and stops when there is no token |
| 2 | the pre-flight: prints the panel `Pre-flight`, then asks Prometheus on `/-/ready`, Loki on `/ready`, Alertmanager on `/-/ready` and Infrahub on `/api/healthcheck` (or, when that fails, on `/health`), in this order; each call waits at most 3 seconds; an answer with a status below 400 counts as ready, and so does an answer from Loki with the status 503 whose text holds `Ingester not ready`; it prints `<service> reachable` for each one and stops at the first that is not ready |
| 3 | a pause |
| 4 | prints the panel of the path: a blank line, the title and one or two lines about the path |
| 5 | path 2 only: sets the maintenance flag of `srl1` to true and prints `srl1.maintenance = true` |
| 6 | posts the alert to the webhook and prints `replayed <status> payload for <device> → <peer>` |
| 7 | prints `Waiting for: <label>`, where `<label>` says what the path proves, and waits for the Loki match; on a match it prints `<label> (Loki match count=<n>)` |
| 8 | path 2 only: prints `Clearing srl1 maintenance back to false.`, sets the flag to false and prints `srl1.maintenance = false` |
| 9 | after paths 1 to 3: a pause |
| 10 | prints the panel `Done`, which names the query `{workflow="<workflow_label>"}` to run in Grafana under Explore, Loki |

When the Infrahub address names `infrahub-server`, nobs uses `http://localhost:8000` instead and prints the rewrite line before any other line, as described in [load-infrahub, schema load and maintenance](#load-infrahub-schema-load-and-maintenance).

The wait: nobs asks Loki at once. When the answer has no match, nobs waits 5 seconds and asks again, until a line matches or the time is up. The time is up `--timeout` seconds after the wait started, by the clock, and the time Loki takes to answer counts. When less than 5 seconds are left, nobs waits only the time that is left, then asks one last time. An ask that has started is not cut off, so one wait can last up to 10 seconds longer than `--timeout`. Each ask looks at the last 5 minutes and counts at most 1000 lines. A Loki that does not answer counts as no match. A line that an earlier run wrote in the last 5 minutes also counts as a match, so a run right after another one can match before the automation has done anything new.

The pause: without `--auto`, nobs prints a blank line and `  [Enter to continue, Ctrl-C to bail] ` and waits for Enter. With `--auto`, it prints nothing and waits 2 seconds. When there is no more input to read, for example with `< /dev/null`, nobs exits with 1 and prints nothing more.

Ctrl-C ends nobs at once, at a pause or at any other moment, and the shell shows the exit code 130. nobs does not clean up first. If you press Ctrl-C on path 2 after the line `srl1.maintenance = true` and before the line `srl1.maintenance = false`, `srl1` stays in maintenance. To clear the flag, run `nobs <name> maintenance -d srl1 --clear`.

| Flag | Variable | Default | What it does |
|---|---|---|---|
| `--auto` | | off | waits 2 seconds at a pause instead of waiting for Enter |
| `--timeout SECONDS` | | 60 | gives how many seconds nobs waits for the Loki match of each path; a whole number from 0 to 18446744073709551615; with 0 nobs asks Loki once; a number too large for the clock of the machine means nobs waits until Loki matches |
| `--prom-url URL` | `PROMETHEUS_URL` | `http://localhost:9090` | gives the address of Prometheus |
| `--loki-url URL` | `LOKI_URL` | `http://localhost:3001` | gives the address of Loki |
| `--am-url URL` | `ALERTMANAGER_URL` | `http://localhost:9093` | gives the address of Alertmanager |
| `--infrahub-url URL` | `INFRAHUB_ADDRESS` | `http://localhost:8000` | gives the address of Infrahub |
| `--webhook-url URL` | `WEBHOOK_URL` | `http://localhost:9997/v1/api/webhook` | gives the address of the webhook |
| `--token TOKEN` | `INFRAHUB_API_TOKEN` | none | gives the token for Infrahub; it is required |

nobs finds each address and the token in this order: the flag, then the variable, then the default, as in [cycle and page](#cycle-and-page). A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. A trailing `/` on the address of Prometheus, Loki, Alertmanager or Infrahub is dropped. The address of the webhook is used as it is given. After the pre-flight, each call to Loki or Infrahub waits at most 10 seconds, and the post to the webhook waits at most 15.

nobs prints a blank line before each panel. For example, this is path 2:

```
$ nobs nobs-t-one try-it --auto

Path 2 - In-maintenance → skip
Mark srl1 as in maintenance, replay an alert, expect decision='skip'.
srl1.maintenance = true
replayed firing payload for srl1 → 10.1.99.2
Waiting for: quarantine flow saw maintenance=true and skipped
quarantine flow saw maintenance=true and skipped (Loki match count=1)
Clearing srl1 maintenance back to false.
srl1.maintenance = false
```

The lines that start with `nobs:` go to standard error (the channel for error messages), and so does the line `<label> - no Loki match after <timeout>s`, which replaces the match line when a wait times out. Everything else goes to standard output. Two problems do not stop the run, and each prints one line to standard error:

| Line | When |
|---|---|
| `nobs: could not POST to webhook: <error>` | the webhook could not be reached or answered with a status of 400 or more; `<error>` is `cannot reach <address>: <reason>` or `the webhook answered <status>: <answer>`, where `<reason>` is why nobs could not connect and `<answer>` is the first 200 characters of the answer; the run goes on and waits for the Loki match, which most likely times out |
| `nobs: could not find srl1 in Infrahub (maintenance toggle skipped)` | Infrahub has no device `srl1`, when nobs sets or clears the flag on path 2; the run goes on |

| Exit code | When |
|---|---|
| 0 | every path found its Loki match |
| 1 | there is no token: `nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).` |
| 1 | a service is not ready in the pre-flight: `nobs: <service> NOT reachable at <url> - bring the stack up first (nobs <name> up)` for Prometheus, Loki or Alertmanager, where `<url>` is the address with `/-/ready` or `/ready`, or `nobs: Infrahub NOT reachable - bring the stack up first (nobs <name> up)`; `<name>` is the `name` setting of the workshop; nobs posts nothing to the webhook |
| 1 | the input is closed at a pause, with no message |
| 1 | Infrahub fails with an error other than a missing device, when nobs sets or clears the flag: `nobs: <error>`, where `<error>` is the text of the failed call; the run ends at once, without the `Done` panel; when the clear failed, `srl1` can stay in maintenance, and you clear it as described in the Ctrl-C paragraph |
| 1 | a wait timed out: `nobs: <n> of 4 waits timed out`, printed after the `Done` panel |
| 2 | a flag nobs does not know, or a value for `--timeout` that is not a whole number from 0 to 18446744073709551615, for example `-1` |

### flap-interface, incident and scenarios

`flap-interface` and `incident` make trouble in the lab on purpose, so the dashboards, the alerts and the flows have something to show. `scenarios` lists what is registered. All three talk to the `sonda-server` of the workshop, and none of them makes a Docker call. A scenario is a body of JSON that tells Sonda what to make, how fast and for how long. `POST /scenarios` starts it, and the server answers with an ID, a name and a state. A cascade is one scenario body with several signals that follow each other, for example an interface that goes down and a BGP session that goes down after it. `flap-interface` and `incident` change the lab, because they start scenarios. They never stop or delete one: a cascade ends by itself when its duration is over.

`flap-interface` posts one cascade for an interface and one cascade for each healthy BGP peer on that link (defined below). The interface cascade flaps the interface: it is up for `--up-duration`, then down for `--down-duration`, again and again until `--duration` is over. While the interface is down, its operational state is held at `2` (down). From `--cascade-delay` after it goes down, a log line with the process `UPDOWN` goes to Loki every 2 seconds. A BGP cascade holds the six BGP metrics of one peer at their down values. It is shifted by `--cascade-delay`, so BGP goes down after the interface and comes back with it. The baseline scenarios of the workshop have a `while:` clause, a rule that lets a scenario run only while a condition on another signal holds. The clause names these cascades, so the baselines pause while a cascade runs and resume when it finishes.

The name of the interface cascade is `<cascade_prefix><device>-intf-<interface>`, with each `/` of the interface written as `-`. The name of a BGP cascade is `<cascade_prefix><device>-bgp-<peer>`, with each `.` of the address written as `-`. `<cascade_prefix>` is the `cascade_prefix` setting of the workshop, and the description of each cascade starts with its `title` setting. For the workshop in the example of this page, the names are `nobs-t-one-cascade-srl1-intf-ethernet-1-1` and `nobs-t-one-cascade-srl1-bgp-10-1-2-2`. The device must be `srl1` or `srl2`: `srl1` uses the metric and label names of gNMI collection, and `srl2` those of SNMP. Sonda refuses a cascade whose name is the name of a scenario that is still active, so wait until the first one has finished before you run the command again.

A healthy peer is an intended BGP peer of the device, in `lab_vars.yml` of the workshop folder, whose address lies inside the IPv4 network of the interface. That network is the address and the prefix length that `lab_vars.yml` gives the interface. A peer outside that network, for example one set up with a wrong address on purpose, never cascades. When no peer fits, nobs prints a warning and flaps the interface alone. With `--no-cascade`, nobs does not read the file and posts the interface cascade alone.

`incident` posts one cascade named `incident-link-failover-<device>`. The primary interface flaps. Once it is down, the utilization of the backup link rises, and once that is above 70 percent, the latency of the backup link grows. The cascade sends its metrics through Prometheus remote write, not through Telegraf, so the label `source="incident-cascade"` stays on them. `link-failover` is the only kind there is. `incident` does not check the device name: it puts it on the signals as it is.

`scenarios` prints the same table as `nobs sonda list`. When there are no scenarios, it prints the header and then `No scenarios registered.`.

| Flag | Variable | Default | Command | What it does |
|---|---|---|---|---|
| `-d`, `--device DEVICE` | | `srl1` | `flap-interface`, `incident` | gives the device; `flap-interface` accepts `srl1` or `srl2` |
| `-i`, `--interface INTERFACE` | | `ethernet-1/1` | `flap-interface` | gives the interface to flap |
| `--primary-interface INTERFACE` | | `ethernet-1/10` | `incident` | gives the interface that flaps |
| `--backup-interface INTERFACE` | | `ethernet-1/1` | `incident` | gives the interface that takes the load |
| `-k`, `--kind KIND` | | `link-failover` | `incident` | gives the kind of cascade; only `link-failover` is accepted |
| `--duration DURATION` | | `4m` for `flap-interface`, `3m` for `incident` | `flap-interface`, `incident` | gives how long the cascade lives; `incident` turns a bare number such as `90` into `90s` |
| `--up-duration DURATION` | | `30s` | `flap-interface` | gives the time the interface stays up before each down phase |
| `--down-duration DURATION` | | `60s` | `flap-interface` | gives the time the interface stays down in each cycle |
| `--cascade-delay DURATION` | | `10s` | `flap-interface` | gives the wait between the interface going down and BGP going down |
| `--no-cascade` | | off | `flap-interface` | posts the interface cascade only |
| `--sonda-url URL` | `SONDA_SERVER_URL` | `http://localhost:8085` | all three | gives the address of `sonda-server` |
| `--sonda-api-key KEY` | `SONDA_API_KEY` | none | `flap-interface`, `incident` | gives the key to send to `sonda-server`; `scenarios` has no flag for it and uses the variable |
| `--loki-url URL` | `SONDA_LOKI_SINK_URL` | `http://loki:3001` | `flap-interface` | gives the address of Loki that Sonda writes the `UPDOWN` log lines to |
| `--prom-url URL` | `SONDA_PROM_REMOTE_WRITE_URL` | `http://prometheus:9090/api/v1/write` | `incident` | gives the address that Sonda sends the metrics to by Prometheus remote write |
| `--follow` | | off | `flap-interface`, `incident` | keeps polling until the scenarios are done |

nobs finds each address and the key in this order: the flag, then the variable, then the default. A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. A trailing `/` on the address of `sonda-server` is dropped. The default is the port of the Sonda that `up` starts, not the `http://localhost:8080` of `nobs sonda`. The addresses of Loki and Prometheus are the ones Sonda uses from inside the Docker network, so they are not the ones you use from the host. With a key, every call sends it as `Authorization: Bearer <key>`: each post, each poll and `scenarios`. nobs sends each body as `application/json`, and each call waits at most 10 seconds.

A duration is a number with a unit: `500ms`, `30s`, `5m` or `1h`. A bare number counts as seconds. `flap-interface` sends `--duration` as you gave it. It reads `--up-duration`, `--down-duration` and `--cascade-delay` to work out the shift of the BGP cascade: the BGP cascade is up for the up time plus the delay, and down for the down time minus the delay, and never less than 0. When one of them is not a duration and nobs builds a BGP cascade, nobs prints `nobs: <text> is not a duration`, exits with 1 and sends nothing. Without a BGP cascade (with `--no-cascade`, or when no peer fits), nobs does not read them and sends them to Sonda as you gave them.

`flap-interface` goes through these steps in order. It stops at the first one that fails.

| Step | What nobs does |
|---|---|
| 1 | checks that the device is `srl1` or `srl2` |
| 2 | reads `lab_vars.yml` and finds the peers, unless `--no-cascade` is given |
| 3 | builds every body; nothing is printed or sent before all of them are built |
| 4 | prints the warning, when there is no healthy peer and `--no-cascade` is not given |
| 5 | prints the line that starts with `Posting` |
| 6 | posts the interface body, then the BGP body of each peer, one after the other |
| 7 | prints the table, then the `registered` line and three hint lines, or follows the scenarios |

`incident` checks the kind, posts its body, prints the table and then the `registered` line and two hint lines, or follows the scenarios. In both examples below, the IDs and the states come from Sonda's answer; the values here are only an example. This is `flap-interface`:

```
$ nobs nobs-t-one flap-interface
Posting inversion cascade for srl1:ethernet-1/1 (peers: 10.1.2.2, duration 4m, cycle 30s up / 60s down) to http://localhost:8085/scenarios
CASCADE       SCENARIO_NAME                              ENTRIES
interface     nobs-t-one-cascade-srl1-intf-ethernet-1-1  5a1f0c2e, 7b3d9e41, 9c2e5a60
bgp 10.1.2.2  nobs-t-one-cascade-srl1-bgp-10-1-2-2       1d4f7a90, 2e5a8b01, 3f6b9c12, 4a7cad23, 5b8dbe34, 6c9ecf45, 7daf0056
registered 10 scenario(s) across 2 cascade body(s); duration 4m
  Baselines auto-resume when cascade reaches `finished` (~4m).
  Inspect:  curl http://localhost:8085/scenarios
  Stop early: curl -X DELETE http://localhost:8085/scenarios/<id>
```

The output and the descriptions call these cascades "inversion" cascades: each one pauses a baseline and sends the down values in its place. The table has one row for each body that nobs posted. `ENTRIES` holds the first 8 characters of the ID of each scenario that Sonda started for that body, or `—` when it started none. The `registered` line counts the scenarios and the bodies. The hint lines hold the address of `sonda-server`; `<id>` is printed as it is, for you to fill in.

And this is `incident`:

```
$ nobs nobs-t-one incident --duration 90
Posting link-failover cascade for srl1 (ethernet-1/10 → ethernet-1/1, duration 90s) to http://localhost:8085/scenarios
ID                                    NAME                              STATE
5a1f0c2e-3b4d-4e6f-8a9b-0c1d2e3f4a5b  interface_oper_state              running
7b3d9e41-5c6e-4f80-9bac-1d2e3f4a5b6c  incident_backup_link_utilization  running
9c2e5a60-7d8f-4a02-8bce-2e3f4a5b6c7d  incident_latency_ms               running
registered 3 scenario(s): 5a1f0c2e-3b4d-4e6f-8a9b-0c1d2e3f4a5b, 7b3d9e41-5c6e-4f80-9bac-1d2e3f4a5b6c, 9c2e5a60-7d8f-4a02-8bce-2e3f4a5b6c7d
  Inspect:  curl http://localhost:8085/scenarios
  Stop:     curl -X DELETE http://localhost:8085/scenarios/<id>
```

When the answer of Sonda has warnings, `incident` prints each one as `  warning: <warning>` after the table.

With `--follow`, nobs prints no `registered` line and no hint lines. After the table it waits 5 seconds, then asks `sonda-server` for the state of each scenario that is not done yet, with `GET <url>/scenarios/<id>`, and prints `  <first 8 characters of the ID> state=<state>` for each one. It repeats this every 5 seconds. A reply with no state counts as `unknown`. A scenario is done at `finished` or `stopped` for `flap-interface`, and at `finished` for `incident`. For `incident`, `--follow` keeps polling until Ctrl-C when a scenario is stopped or deleted before it finishes. When a poll fails, nobs prints `nobs: poll error on <id>: <error>` and asks again in the next round. When every scenario is done, `flap-interface` prints `all cascade scenarios finished or stopped; baselines back to running`, `incident` prints `all scenarios completed`, and nobs exits with 0. Ctrl-C ends nobs at once. It stops the follow only: the scenarios keep running until their duration is over.

The lines that start with `nobs:` and the lines that start with `  warning:` go to standard error. Everything else goes to standard output.

| Line | When |
|---|---|
| `nobs: unknown device <device>; expected one of srl1, srl2` | `flap-interface` was given another device; nothing is read or sent |
| `nobs: lab_vars file not found: <path>` | `flap-interface` could not find `lab_vars.yml` in the workshop folder |
| `nobs: cannot read <path>: <reason>` | `flap-interface` could not read or parse `lab_vars.yml` |
| `nobs: <text> is not a duration` | `flap-interface` was given a time it cannot read, see above |
| `nobs: no healthy BGP peers mapped to <device>:<interface>; running interface flap only.` | a warning: no peer fits the link, and the command goes on with the interface cascade |
| `nobs: unknown kind <kind>; supported: link-failover` | `incident` was given another kind; nothing is sent |
| `nobs: POST <scenario name> failed: <error>` | `flap-interface` could not post a body; the cascades posted before stay |
| `nobs: POST /scenarios failed: <error>` | `incident` could not post its body |
| `nobs: unexpected response shape: <body>` | Sonda answered with a status below 400 but the answer is not a scenario or a list of scenarios, or, for `incident`, a list with none in it; `<body>` is the first 300 characters of the answer |
| `nobs: poll error on <id>: <error>` | a poll of `--follow` failed; nobs tries again in the next round |
| `nobs: cannot reach <url>: <reason>` | `scenarios` could not reach `sonda-server` |
| `nobs: sonda answered <status>: <body>` | `scenarios` got a status of 400 or more |
| `nobs: sonda answered a list nobs cannot read: <reason>` | `scenarios` got an answer that is not a list of scenarios |

In the post and poll lines, `<error>` is `cannot reach <url>: <reason>` or `sonda answered <status>: <body>`, where `<reason>` is why nobs could not connect and `<body>` is the answer as Sonda sent it.

| Exit code | When |
|---|---|
| 0 | the command did what it was asked; with `--follow`, every scenario is done |
| 1 | any `nobs:` line in the table above except the warning and the poll error |
| 2 | a flag nobs does not know |

### reset

`reset` returns the lab to the state every part of the workshop starts from: the two broken BGP peers fire within about a minute, no device is in maintenance, none of the scenarios that step 5 deletes is left in Sonda, no workshop silence is left in Alertmanager, Infrahub holds the two workshop devices, and the log sink of Sonda for `srl1` delivers to Loki. The baseline scenarios are the scenarios that the `sonda-setup` container of the workshop posts to `sonda-server`; they send the normal metrics and log lines of the lab. The log sink is the part of Sonda that sends log lines to Loki. Run `reset` at the start of a session and between parts. You can also run it on a lab that is already clean. It changes the lab: it can load Infrahub, change the `maintenance` flag, expire silences, delete scenarios and restart containers.

`reset` goes through eight steps in this order. When a step fails, `reset` prints a warning and goes on with the next step.

| Step | What nobs does |
|---|---|
| 1 | asks Infrahub for its devices of the kind `WorkshopDevice`; when there are fewer than two, it loads the schema file and `lab_vars.yml` as `load-infrahub` does |
| 2 | clears the `maintenance` flag of `srl1`, then of `srl2`, and tells Sonda about each one, as `maintenance --clear` does |
| 3 | runs `docker compose` with `up sonda-setup`, and waits until it has finished; `sonda-setup` is the container of the workshop's compose file that posts the baseline scenarios to `sonda-server` and then exits |
| 4 | expires the active silences of Alertmanager whose comment or creator contains one of the `silence_hints` of the workshop |
| 5 | deletes every Sonda scenario named `cascade_active`, `incident_backup_link_utilization`, `incident_latency_ms` or `updown_logs_down`; `flap-interface` and `incident` start scenarios with these names, and one name can belong to more than one scenario; `reset` does not delete the other scenarios that these two commands start |
| 6 | looks for baseline scenarios that are stuck, and when it finds one, restarts `sonda-server`, waits 5 seconds and runs `sonda-setup` again |
| 7 | unless `--skip-sonda-logs` is given, asks Loki for one log line of `srl1` from the last minute, and restarts `sonda-logs` when there is none |
| 8 | prints the last line, but only when no step printed a warning |

A baseline is stuck when its state is `paused` and the name in the `pending_ref` of its scenario starts with the `cascade_prefix` setting or with `incident-link-failover-`. `pending_ref` is the field of a scenario in `sonda-server` that names the scenario it waits for. After step 5, a baseline should run again by itself, and sometimes it does not. In step 7, the question to Loki is `{device="srl1", pipeline="direct"}`. No line in the last minute means that the log sink of Sonda does not deliver, and a restart of `sonda-logs` resets it.

| Flag | Variable | Default | What it does |
|---|---|---|---|
| `--alertmanager-url URL` | `ALERTMANAGER_URL` | `http://localhost:9093` | gives the address of Alertmanager |
| `--sonda-url URL` | `SONDA_SERVER_URL` | `http://localhost:8085` | gives the address of `sonda-server` |
| `--loki-url URL` | `LOKI_URL` | `http://localhost:3001` | gives the address of Loki |
| `--infrahub-url URL` | `INFRAHUB_ADDRESS` | `http://localhost:8000` | gives the address of Infrahub |
| `--token TOKEN` | `INFRAHUB_API_TOKEN` | none | gives the token for Infrahub |
| `--skip-sonda-logs` | | off | leaves out step 7, so nobs does not ask Loki and does not restart `sonda-logs` |

nobs finds each address and the token in this order: the flag, then the variable, then the default. A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. A trailing `/` on an address is dropped. A token you give with `--token` is on the command line of nobs itself, and a list of processes shows it, so keep the token in the `.env` file. When the address of Infrahub names `infrahub-server`, nobs uses `http://localhost:8000` and prints the rewrite line `INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000` after the first line, as `load-infrahub` does.

Two more variables apply. `SONDA_API_KEY` is the key that nobs sends to Sonda as `Authorization: Bearer <key>` in steps 2, 5 and 6; it has no flag and no default. `SONDA_LOKI_SINK_URL` is the address of Loki as Sonda sees it, and its default is `http://loki:3001`; step 2 sends it to Sonda, as `maintenance` does. Each of the two counts as not given when it is empty or only spaces.

Each call to Infrahub, Alertmanager, Sonda and Loki has a time limit:

| Call | Waits at most |
|---|---|
| every call not named below | 10 seconds |
| the message to Sonda in step 2 | 35 seconds |
| the schema load of step 1 | 120 seconds |
| the wait of step 1 until Infrahub shows the three kinds of node | 30 seconds |

The `docker` calls have no time limit. They run with the project name and the compose file of the workshop, like the other commands that call `docker`. nobs does not show the standard output of `docker`, and it shows the standard error. When the compose file is missing, every `docker` call gives a warning, and the steps that need no `docker` go on.

`reset` checks the token first. When there is none, it prints the first line, then the rewrite line when there is one, then the error `nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).`, and exits with 1: it sends nothing, runs no `docker` call and prints no last line.

This is the output for a lab that needed every step. Your lab prints the lines of steps 1 to 7 that apply to it:

```
$ nobs nobs-t-one reset
Resetting lab to known-good state.
Infrahub has 0 WorkshopDevice record(s); re-loading schema + data
  re-loaded Infrahub schema + data (2 device(s) seeded)
  cleared maintenance on: srl1, srl2
  re-applied sonda-server baseline scenarios (idempotent)
  expired 1 workshop-related Alertmanager silence(s)
  deleted 4 lingering cascade scenario(s)
detected 1 wedged baseline(s) after cascade delete; restarting sonda-server
  restarted sonda-server and re-applied baselines
sonda-logs srl1 streams look wedged (no Loki activity in last 60s); restarting
  restarted sonda-logs (loki sink reset)
lab reset complete — both broken peers should be firing within ~60s
```

Between these lines you also see what `docker compose` writes to standard error during the `docker` calls of steps 3, 6 and 7, for example ` Container sonda-setup Started` and lines that start with `sonda-setup  |`. The example leaves them out. When the address of Infrahub names `infrahub-server`, the rewrite line comes second. On a lab that is already clean, the lines of nobs are the first line, `  Infrahub already loaded (2 devices)`, the `cleared` line, the `re-applied` line and the last line. In these lines, "wedged" means stuck, "lingering" means left over, "idempotent" means that running it again changes nothing, and `~60s` means about 60 seconds. The lines have these meanings. Everything in the table goes to standard output.

| Line | When |
|---|---|
| `Resetting lab to known-good state.` | always the first line |
| `  Infrahub already loaded (<n> devices)` | step 1 found two or more devices |
| `Infrahub has <n> WorkshopDevice record(s); re-loading schema + data` | step 1 found fewer than two; nothing of the load itself is printed |
| `  re-loaded Infrahub schema + data (<n> device(s) seeded)` | the load worked; `<n>` is the number of devices nobs created or updated |
| `  cleared maintenance on: <devices>` | step 2 set the flag to false on at least one device, also when it was already false; `<devices>` are the names, separated by `, ` |
| `  re-applied sonda-server baseline scenarios (idempotent)` | step 3 worked |
| `  expired <n> workshop-related Alertmanager silence(s)` | step 4 expired at least one silence |
| `  deleted <n> lingering cascade scenario(s)` | step 5 deleted at least one scenario |
| `detected <n> wedged baseline(s) after cascade delete; restarting sonda-server` | step 6 found at least one stuck baseline |
| `  restarted sonda-server and re-applied baselines` | the restart and the second `sonda-setup` of step 6 worked |
| `sonda-logs srl1 streams look wedged (no Loki activity in last 60s); restarting` | step 7 found no log line |
| `  restarted sonda-logs (loki sink reset)` | the restart of step 7 worked |
| `lab reset complete — both broken peers should be firing within ~60s` | the last line, printed only when no warning was printed |

When nobs cannot expire a silence or delete a scenario, it prints nothing and does not count it. The warnings go to standard error. Each one starts with `nobs: `.

| Line | When |
|---|---|
| `nobs: Infrahub probe failed: <error> — skipping load check` | step 1 could not ask Infrahub, and ends |
| `nobs: load-infrahub failed: <error>` | the load of step 1 failed; `<error>` is the text `load-infrahub` prints after `nobs: `, for example `lab_vars file not found: <path>` |
| `nobs: maintenance --clear failed for <device>: <error>` | step 2 failed for that device; `<error>` is the text `maintenance` prints after `nobs: `, for example `<kind> <device> not found in Infrahub.` or `sonda /events post failed: <error>` |
| `nobs: <call> failed: exited with code <n>` | `docker` ended with a code other than 0; `<call>` is `sonda-setup up`, `sonda-server restart` or `sonda-logs restart`; after a failed `sonda-server restart`, nobs does not run `sonda-setup` again |
| `nobs: <call> failed: cannot run docker: <error>` | `docker` could not start |
| `nobs: compose file not found: <path>` | the compose file is missing; nobs prints this line for each `docker` call it would make |
| `nobs: Alertmanager fetch failed: <error> — skipping silence cleanup` | step 4 could not read the silences, and ends |
| `nobs: Sonda /scenarios fetch failed: <error> — skipping cascade cleanup` | step 5 could not read the list of scenarios, and ends |
| `nobs: Sonda /scenarios fetch failed: <error> — skipping wedge check` | step 6 could not read the list of scenarios, and ends |
| `nobs: Loki probe failed: <error> — skipping sonda-logs check` | step 7 could not ask Loki, and ends |

In these lines, `<error>` is the text of the failed call. For Infrahub it is `cannot reach <url>: <reason>` or `Infrahub answered <status> on <path>: <body>`. For Alertmanager it is `cannot reach Alertmanager at <url>: <reason>` or `Alertmanager at <url> answered <status>`. For Sonda it is `cannot reach <url>: <reason>`, `sonda answered <status>: <body>` or `sonda answered a list nobs cannot read: <reason>`. For Loki it is `cannot reach <url>: <reason>` or `Loki answered <status> on <path>: <body>`. `<reason>` is why nobs could not connect, and `<body>` is the answer as the server sent it.

| Exit code | When |
|---|---|
| 0 | nobs printed no warning; the last line is printed |
| 1 | there is no token: `nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).` |
| 1 | nobs printed at least one warning; the last line is not printed |
| 2 | a flag nobs does not know |

### preflight

`preflight` is the workshop's own check of the running stack. Run it before a session, after `up` and `load-infrahub`. It has three layers, A, B and C, and nobs runs A and B. Layer A checks the data shape: it waits until Prometheus and Loki hold the series and the log lines the workshop needs, makes events on purpose, and checks that Loki holds them. A series is one metric with one set of label values. Layer B asks Grafana for the data of every panel of every dashboard of the workshop. A panel is one chart or table of a dashboard. Layer C, a screenshot of every panel, is not built in nobs. `nobs preflight` is a different command: it checks the machine, not the stack (see [Setup and preflight](#setup-and-preflight)).

`preflight` changes the lab, and it leaves `srl1` in maintenance. Its layer A starts scenarios in Sonda with `flap-interface` and sets the `maintenance` flag of `srl1` to true, with the same calls as `maintenance`, so Infrahub and Sonda both hear of it. While the flag is true, the policy skips the alerts of `srl1`. Run `nobs <workshop> reset` after it to clear the flag and to delete the scenarios it left.

`preflight` goes through these steps in order. A failure in steps 2 to 4 ends the run before nobs sends any request. After step 4, only a file that cannot be written ends the run.

| Step | What nobs does |
|---|---|
| 1 | prints the rewrite line, when the address of Infrahub names `infrahub-server`, as `reset` does |
| 2 | checks that there is a token |
| 3 | creates the output folder, and its parents, when they are missing |
| 4 | reads every `.json` file in the `grafana/dashboards` folder of the workshop folder |
| 5 | prints an empty line and `Layer A — data shape waits`, runs layer A and writes `layer_a.json` |
| 6 | prints `Layer B — per-panel /api/ds/query`, runs layer B and writes `layer_b.json` |
| 7 | prints the line about layer C |
| 8 | writes `REPORT.md`, then prints the summary line and the report line |

Layer A goes through these steps in order. A wait asks again every 4 seconds, until the answer is right or the time is over. A wait that is not right in time is a failure, and layer A goes on with the next step.

| Step | What nobs does | Waits at most |
|---|---|---|
| 1 | asks Prometheus for `count(bgp_oper_state{pipeline=~".+"})` until it is at least 6 | 180 seconds |
| 2 | asks Prometheus for `count(interface_oper_state{intf_role="peer", pipeline=~".+"})` until it is at least 6 | 180 seconds |
| 3 | asks Loki for the series of `{vendor_facility_process="UPDOWN"}` in the last 5 minutes until the label `pipeline` holds both `direct` and `vector` | 180 seconds |
| 4 | runs `flap-interface --device srl1 --interface ethernet-1/1 --no-cascade --duration 30s --up-duration 3s --down-duration 3s --cascade-delay 10s` and prints its lines as it prints them | no wait |
| 5 | sets the `maintenance` flag of `srl1` to true and tells Sonda, and prints the lines of `maintenance` | no wait |
| 6 | sleeps 130 seconds, so the two-minute windows (`[2m]`) in the queries of the dashboards can fill with the new events, and prints `  ... <n>s left` every 20 seconds | 130 seconds |
| 7 | asks Loki for the lines of `{vendor_facility_process="UPDOWN"}` in the last 5 minutes, at most 200, until there are at least 8 | 30 seconds |
| 8 | asks Loki for the lines of `srl1` with the process `UPDOWN` and a text that matches `Interface .* changed state`, in the last 5 minutes, until there are at least 4 | 30 seconds |
| 9 | asks Loki for `{device="srl1", source="workshop-trigger", event="config-push"}` in the last 5 minutes until there is at least 1 line | 30 seconds |

Layer A exits with 1 when a wait failed, or when step 4 or step 5 exited with a code other than 0, and the summary line then says `preflight failures`. Step 4 and step 5 print their own `nobs:` line when they fail, and nobs prints no other line for them.

Layer B reads the dashboard files in the order of their names. It takes every panel that is not a `row`, also the panels inside a row, in the order of the file. The targets of a panel are its queries. When the targets of a panel hold `$device` or `${device}`, nobs asks for the panel twice, with the device `srl1` and then with `srl2`. Otherwise it asks once, and prints the device as `(no template)` and uses `srl1` in the query. For each target, nobs sends one `POST /api/ds/query` to Grafana, for the last 5 minutes, and waits at most 15 seconds for the answer. Grafana answers with frames: a frame is one block of data, and a row is one value in its first column. nobs turns each answer into one text. It checks the answer in the order of the table below, and the first row that fits gives the text.

| Answer | When |
|---|---|
| the text of the error, for example `cannot reach <url>: <reason>` | nobs got no answer from Grafana |
| `http <status>` | Grafana answered with a status other than 200 |
| `no results` | the answer has no `results` |
| `ds error: <error>` | the first result has a status of 400 or more, or an `error`; `<error>` is that error, else the status |
| `0 frames` | the first result has no frames |
| `<n> frames, 0 rows` | the frames hold no row; `<n>` is the number of frames that hold values |
| `<n> frames, <m> rows` | the frames hold `<m>` rows; this is the only good answer |

A target is good when its answer is the last one. A panel passes when one of its targets is good, fails when none is, and is skipped when it has no target. Layer B exits with 1 when a panel failed, and with 0 otherwise.

| Flag | Variable | Default | What it does |
|---|---|---|---|
| `--skip-c` | | off | is accepted and changes nothing, because layer C is not built |
| `--out-dir DIR` | `PREFLIGHT_OUT_DIR` | `/tmp/preflight-out` | gives the folder for the three files |
| `--prom-url URL` | `PROMETHEUS_URL` | `http://localhost:9090` | gives the address of Prometheus |
| `--loki-url URL` | `LOKI_URL` | `http://localhost:3001` | gives the address of Loki |
| `--grafana-url URL` | `GRAFANA_URL` | `http://localhost:3000` | gives the address of Grafana |
| `--sonda-url URL` | `SONDA_SERVER_URL` | `http://localhost:8085` | gives the address of `sonda-server` |
| `--infrahub-url URL` | `INFRAHUB_ADDRESS` | `http://localhost:8000` | gives the address of Infrahub |
| `--token TOKEN` | `INFRAHUB_API_TOKEN` | none | gives the token for Infrahub |

nobs finds each address, the folder and the token in this order: the flag, then the variable, then the default. A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. A trailing `/` on an address is dropped. `--with-c` is not accepted: it is a usage error, and nobs exits with 2. A token you give with `--token` is on the command line of nobs itself, and a list of processes shows it, so keep the token in the `.env` file.

Four more variables apply, and none has a flag. `GRAFANA_USER` and `GRAFANA_PASSWORD` are the login that nobs sends to Grafana; both default to `admin`, and nobs never prints them. `SONDA_API_KEY` is the key that step 4 and step 5 of layer A send to Sonda; it has no default. `SONDA_LOKI_SINK_URL` is the address of Loki as Sonda sees it, its default is `http://loki:3001`, and steps 4 and 5 send it to Sonda; it is not the same as `--loki-url`, which is the address nobs itself uses. Each of the four counts as not given when it is empty or only spaces.

`preflight` writes three files into the output folder, and writes over files of the same names. It writes each file as soon as its layer is done, so a later failure leaves the earlier files in place.

| File | What it holds |
|---|---|
| `layer_a.json` | a list with one entry for each wait of layer A: `label`, `ok`, `elapsed_s`, `detail` and `expected` |
| `layer_b.json` | a list with one entry for each dashboard: its title, its file and its panels, each with its device, its status and the answer of each target |
| `REPORT.md` | the title and the time (UTC), the exit code of each layer, one line for each wait and one table for each dashboard |

This is the output of a run where every check passed. The IDs, the counts and the times come from your stack; the values here are only an example.

```
$ nobs nobs-t-one preflight

Layer A — data shape waits
Layer A — wait for data shape
  [  0.1s] OK   bgp_oper_state pipeline convergence — count = 6
  [  0.1s] OK   interface_oper_state intf_role=peer convergence — count = 6
  [  4.1s] OK   logs pipeline convergence (UPDOWN) — pipelines = direct, vector
Layer A — flap-interface (in-process, --no-cascade)
Posting inversion cascade for srl1:ethernet-1/1 (peers: no-cascade, duration 30s, cycle 3s up / 3s down) to http://localhost:8085/scenarios
CASCADE    SCENARIO_NAME                              ENTRIES
interface  nobs-t-one-cascade-srl1-intf-ethernet-1-1  5a1f0c2e, 7b3d9e41, 9c2e5a60
registered 3 scenario(s) across 1 cascade body(s); duration 30s
  Baselines auto-resume when cascade reaches `finished` (~30s).
  Inspect:  curl http://localhost:8085/scenarios
  Stop early: curl -X DELETE http://localhost:8085/scenarios/<id>
Layer A — maintenance toggle (in-process)
srl1.maintenance: false → true
The next alert for this device will be SKIPPED by the policy.
Layer A — sleeping 130s for [2m] windows to fill
  ... 130s left
  ... 110s left
  ... 90s left
  ... 70s left
  ... 50s left
  ... 30s left
  ... 10s left
  [  0.1s] OK   Loki has UPDOWN events from both producers — events last 5m = 12
  [  0.1s] OK   Interface Flap annotation source has lines — flap lines = 8
  [  0.1s] OK   Device Config Push annotation source has lines — config-push lines = 1

Layer A — wrote /tmp/preflight-out/layer_a.json
Layer B — per-panel /api/ds/query

=== Workshop One (one.json) — 5 panels ===
  [OK  ] panel #1 timeseries     device=srl1         'BGP sessions'                           → prometheus:1 frames, 3 rows
  [OK  ] panel #1 timeseries     device=srl2         'BGP sessions'                           → prometheus:1 frames, 3 rows
  [OK  ] panel #2 logs           device=(no template) 'Interface events'                       → loki:1 frames, 3 rows
  [OK  ] panel #4 table          device=srl1         'Device record'                          → yesoreyeram-infinity-datasource:1 frames, 3 rows
  [OK  ] panel #4 table          device=srl2         'Device record'                          → yesoreyeram-infinity-datasource:1 frames, 3 rows
  [OK  ] panel #5 stat           device=(no template) 'Lab is up'                              → prometheus:1 frames, 3 rows; prometheus:1 frames, 0 rows
  [SKIP] panel #6 text           device=(no template) 'Notes'                                  → no targets

Layer B summary: PASS=6 FAIL=0 SKIP=1
Layer B — wrote /tmp/preflight-out/layer_b.json
Layer C — per-panel screenshots: not built in nobs, skipped
preflight green (136.8s) — Layer A exit=0 | Layer B exit=0 | Layer C skipped
report: /tmp/preflight-out/REPORT.md
```

The `flap-interface` and `maintenance` lines are the lines of those commands, and their IDs and the first value in the `maintenance` line come from Sonda and Infrahub. A panel line is `[OK  ]`, `[FAIL]` or `[SKIP]`, the panel's ID in the dashboard file, the type of the panel, the device, the title in quotes, an arrow, and the answer of each target as `<type>:<answer>`, joined by `; `. A data source is the service that Grafana reads the data from. `<type>` is the type of that data source, for example `prometheus` or `loki`. `<answer>` is one of the answers in the table above. A panel with no target prints `no targets` after the arrow. nobs cuts the title to 38 characters and the text after the arrow to 140. These are the lines of nobs itself:

| Line | When |
|---|---|
| `Layer A — data shape waits` | after the empty line, before layer A |
| `Layer A — wait for data shape` | the first line of layer A, before the waits of steps 1 to 3 |
| `  [<s>s] OK   <label> — <detail>` | a wait was right; `<s>` is the seconds it took, with one decimal |
| `  [<s>s] FAIL <label> — <detail>` | a wait was not right in time; `<detail>` is the last answer, else the last error, else `no data` |
| `Layer A — flap-interface (in-process, --no-cascade)` | before step 4; in-process means that nobs runs `flap-interface` itself, not as a second program |
| `Layer A — maintenance toggle (in-process)` | before step 5 |
| `Layer A — sleeping 130s for [2m] windows to fill` | before step 6 |
| `Layer A — wrote <path>` | `layer_a.json` is written |
| `Layer B — per-panel /api/ds/query` | the start of layer B |
| `=== <title> (<file>) — <n> panels ===` | after an empty line, at the start of each dashboard; `<title>` is the `title` in the file, else the file name without `.json`; `<n>` counts panels, not lines, so a panel with `$device` gives two lines |
| `Layer B summary: PASS=<p> FAIL=<f> SKIP=<s>` | the end of layer B |
| `Layer B — wrote <path>` | `layer_b.json` is written |
| `Layer C — per-panel screenshots: not built in nobs, skipped` | always, with or without `--skip-c` |
| `preflight green (<s>s) — Layer A exit=<a> \| Layer B exit=<b> \| Layer C skipped` | both exit codes are 0; `<s>` is the seconds since step 4 passed, with one decimal |
| `preflight failures (<s>s) — Layer A exit=<a> \| Layer B exit=<b> \| Layer C skipped` | at least one exit code is not 0 |
| `report: <path>` | the last line, after the summary line |

These lines start with `nobs:` and go to standard error, as do the `nobs:` lines of steps 4 and 5 of layer A. Everything else goes to standard output.

| Line | When |
|---|---|
| `nobs: INFRAHUB_API_TOKEN is required (set it in .env or pass --token).` | there is no token; nothing is sent |
| `nobs: cannot write <path>: <error>` | the folder or a file cannot be written; nobs stops there, so it prints no `wrote` line for that file and no summary |
| `nobs: cannot read <path>: <error>` | the dashboards folder, or a file in it, cannot be read, or the file is not JSON |
| `nobs: no dashboard files in <path>` | the dashboards folder holds no `.json` file |

The whole run takes up to about 13 minutes on a stack that has just started: three waits of up to 180 seconds, 130 seconds of sleep, three waits of up to 30 seconds, and then layer B. On a stack that is ready, the waits are right at once, and the run takes about 130 seconds plus the time of layer B.

| Exit code | When |
|---|---|
| 0 | both layers passed |
| 1 | a wait failed, a panel failed, step 4 or step 5 of layer A failed, or one of the four `nobs:` lines above was printed |
| 2 | a flag nobs does not know, `--with-c` included |

## Setup and preflight

`nobs preflight` checks that this machine can run a workshop. It prints one line per check: the name of the check, `ok`, `warn` or `fail`, and a detail. It exits with 1 when any check fails, and with 0 when none does. A `warn` does not change the exit code. `nobs <workshop> preflight` is a different command: it is the workshop's own check of the running stack (see [preflight](#preflight)).

A GiB (gibibyte) is 1,073,741,824 bytes, a little more than a GB. nobs compares the exact size with the limits below, and prints the whole GiB, rounded down.

| Check | What nobs looks at | Result |
|---|---|---|
| `docker` | `docker --version` | ok; fail when it cannot run or exits with a non-zero code |
| `compose` | `docker compose version` | same as `docker` |
| `memory` | `sysctl -n hw.memsize` on macOS, `/proc/meminfo` on Linux | ok at 8 GiB or more; warn from 6 GiB to just under 8 GiB; fail under 6 GiB; warn `memory not checked` when nobs cannot read it |
| `disk` | `df -k` of the current folder | ok at 5 GiB free or more; fail under 5 GiB; warn `disk not checked` when nobs cannot read it |
| `network` | a connection (TCP, the way HTTPS starts) to port 443 of five hosts, one after another; nobs waits up to 3 seconds for each connection | ok when all answer; warn when some do not; never fail |

The five hosts are `ghcr.io`, `registry-1.docker.io`, `github.com`, `registry.opsmill.io` and `grafana.com`. For `network`, the detail is `5 hosts answered`, or `no answer from` and the hosts that did not answer. For `docker` and `compose`, the detail is the first line the program printed, or `exited with code <N>`, or the error text when the program cannot start.

For example:

```
$ nobs preflight
docker   ok    Docker version 27.0.1, build 4c2d2b1
compose  ok    Docker Compose version v2.29.1
memory   ok    36 GiB
disk     ok    201 GiB free
network  warn  no answer from registry.opsmill.io
```

Each line is the check name padded to 7 characters, the status padded to 4, and the detail, with two spaces between them.

`nobs setup` runs `nobs preflight` first. When a check fails, it stops with exit 1 and changes nothing. Otherwise it does what `nobs <workshop> setup` does, for every workshop that `nobs workshops` would list. It prints one line per workshop: `<name>: created <folder>/.env from .env.example`, `<name>: kept <folder>/.env` or `<name>: no .env.example in <folder>`. It skips and stops on a `nobs.toml` it cannot read in the same way as `nobs workshops`; when it stops, it creates no `.env` and exits with 1. When a copy fails, nobs prints the problem, stops there and exits with 1; the workshops before it keep their new `.env`. `nobs setup` and `nobs preflight` read the `.env` file of the current folder.

## Sonda

Sonda makes test metrics and logs, and `sonda-server` is its HTTP server. The `nobs sonda` commands talk to a running `sonda-server`. Each command is one HTTP call, and nobs prints what the server answers.

A scenario is a YAML file that tells Sonda what to make, how fast, for how long and where to send it (the place it sends to is called the sink). The server gives each scenario it starts an ID. `load` and `list` print it, and `show`, `stats` and `delete` take it as `ID`. An event is one metric or log line that the server sends once to a sink and then confirms.

| Command | HTTP call | What it prints |
|---|---|---|
| `nobs sonda health` | `GET /health` | the reply |
| `nobs sonda ready` | `GET /ready` | the reply |
| `nobs sonda list` | `GET /scenarios` | a table with the columns `ID`, `NAME`, `STATE`, `ELAPSED` (seconds) and `DEGRADED`; only the header line when there are no scenarios; `-` for a field the server left out |
| `nobs sonda show ID` | `GET /scenarios/ID` | the reply |
| `nobs sonda stats ID` | `GET /scenarios/ID/stats` | the reply |
| `nobs sonda delete ID` | `DELETE /scenarios/ID` | the reply |
| `nobs sonda load FILE` | `POST /scenarios` | the reply |
| `nobs sonda event FILE` | `POST /events` | the reply |
| `nobs sonda metrics` | `GET /metrics` | the metrics text as the server sent it |
| `nobs sonda version` | `GET /metrics` | `sonda-server <version> <git_sha>`, read from the `sonda_server_build_info` line; `<git_sha>` is `unknown` when the server does not know it, as an older server such as 1.22.2 does |

The server address comes from `--server URL`, then the variable `SONDA_SERVER_URL` in the shell, then the same variable in the `.env` file of the current folder, then `http://localhost:8080`. An empty value counts as not set, so nobs moves to the next place. The flags `--server` and `--api-key` can go before or after the command, for example `nobs sonda --server http://127.0.0.1:8085 list`.

The key comes from `--api-key KEY`, then the variable `SONDA_API_KEY` in the shell, then the same variable in the `.env` file, in the same way as the address. An empty value counts as not set here too. With a key, every call except `health` and `ready` sends it as `Authorization: Bearer <key>`. With no key, nobs sends none. nobs never writes the key anywhere.

nobs talks plain HTTP only, so an address that starts with `https://` does not work yet.

When the reply is JSON (a text format for data), nobs prints it with one field per line and the fields sorted by name. `list` prints a table instead, and an error reply prints as received.

A scenario file names its type on a line that begins with `kind:`, with nothing before it on the line. The value `runnable` means a scenario the server can run. The value `composable` means a pack, a reusable set of metric definitions that other scenario files refer to and that is not run on its own. `nobs sonda load FILE` sends a file only when it has a `kind:` line with the value `runnable`. For any other file, including a JSON file, nobs prints the reason, exits with 1, and sends nothing.

When the server answers with a status of 400 or more, nobs prints `nobs: sonda answered <status>: <body>` and exits with 1. When the server cannot be reached, or no reply comes within 10 seconds (35 seconds for `event`), nobs prints `nobs: cannot reach <url>: <error>` and exits with 1.

## Screen

`nobs tui` opens a screen in the terminal with the scenarios of a running `sonda-server`. The screen asks the server for the list every second. It never waits for the server, so a key works at once even when the server does not answer.

The screen has four parts:

| Part | What it shows |
|---|---|
| top line | the server address; `refreshed <n>s ago`, the age of the last good answer, or `no answer yet`; the last error and its age, `(<n>s ago)`, when there is one |
| left table | the scenarios, with the columns `ID` (its first 8 characters), `NAME`, `STATE`, `ELAPSED` (seconds) and `DEGRADED`; `>` marks the selected scenario |
| right panel | what `nobs sonda stats` prints for the selected scenario, as `field  value` lines sorted by name (a text value keeps its quotes, for example `state  "running"`), then what `nobs sonda show` prints for it; the lines that do not fit are cut off |
| bottom line | the keys; or the open question, `load: <path>` or `delete <name>? y/n`; or, for three seconds, the answer to a load or a delete |

A state is shown as the server sends it, for example `running`, `paused` or `finished`.

| Key | What it does |
|---|---|
| `j` or down arrow | select the next scenario |
| `k` or up arrow | select the previous scenario |
| `r` | ask the server for the list now |
| `l` | ask for the path of a scenario file: type it, then press `Enter` to load it or `Esc` to cancel; `Backspace` removes the last character |
| `d` | ask `delete <name>? y/n` about the selected scenario: `y` deletes it, any other key cancels |
| `q` or `Esc` | close the screen and give the terminal back as it was |

While a question is open (the path after `l`, or the delete question after `d`), the keys answer the question: `Esc` closes it, and `q` does not close the screen (in the path, `q` is typed like any other letter; in the delete question, any key but `y` cancels).

The flags `--server URL` and `--api-key KEY` work as they do for `nobs sonda`, and nobs looks for the address and the key in the same places (see [Sonda](#sonda)).

`nobs tui` needs no lab folder. Like `nobs sonda`, it works from any folder and ignores `--dir` and `NOBS_DIR`.

When the server stops answering, or no reply comes within 10 seconds, the screen keeps the last good list and stats. The top line then shows how old they are, the error and how old the error is.

`l` sends a file only when `nobs sonda load` would send it. A pack, or any other file that is not `runnable`, is refused, and the bottom line shows `nobs: <path>: <reason>`. A path that does not start with `/` is read from the folder where you started nobs.

`nobs tui` needs a terminal. When its input or output is not a terminal, for example a pipe or a file, it prints `nobs: the screen needs a terminal` and exits with 1. When the terminal fails while the screen is open, nobs gives the terminal back, prints `nobs: the screen failed: <error>` and exits with 1. After `q` or `Esc` it exits with 0.
