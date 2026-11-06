# nobs

A terminal program (a text user interface) for network labs and observability workshops. It replaces the labby command line tool and takes in the lab tooling of labcli, the workshops and the network observability lab.

Status: early. The `nobs docker` group (13 commands), the `nobs containerlab` group (3 commands), the `nobs sonda` group (10 commands), `nobs workshops` and `nobs <workshop> ps` work. The planning papers (requirements, findings, decisions and the plan) are not kept in this repository.

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

These commands do not need a lab folder. They work from any folder and ignore `--dir` and `NOBS_DIR`: `nobs docker network`, `nobs sonda`, a `nobs docker` command given `--compose FILE`, and a `nobs containerlab` command given a `TOPOLOGY` file. `nobs workshops` does not need a lab folder either, but it uses `--dir` and `NOBS_DIR` when they are set (see [Workshops](#workshops)).

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

The commands of nobs itself (`docker`, `containerlab`, `sonda`, `workshops`, `setup`, `preflight` and `help`) keep their meaning in every folder. So inside a workshop, `nobs setup` is still the setup of every workshop (see [Setup and preflight](#setup-and-preflight)), not `nobs <workshop> setup`.

When the first word is not a workshop name, and you are not inside a workshop, nobs prints the error below and exits with 2. The second line lists the names nobs did find, and is left out when there are none.

```
nobs: no workshop named nobs-t-none
known: nobs-t-two
```

`nobs <workshop> --help` lists the commands of a workshop. There are fifteen. Every one except `setup`, `status`, `alerts`, `silences`, `load-infrahub`, `schema load` and `maintenance` runs `docker compose --project-name <project> -f <folder>/docker-compose.yml`, then the words in the second column below. `<project>` is the `project` setting in the workshop's `nobs.toml`, and `<folder>` is the workshop folder. `SERVICES` are service names from the compose file; when you give none, the command acts on all of them.

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

When a command makes two calls, nobs stops at the first one that fails. A failed `docker` call makes nobs exit with the code of that call, and `up` and `restart` print no addresses then. When `docker` cannot start, nobs prints `nobs: cannot run docker: <error>` and exits with 1. When the compose file does not exist, nobs prints `nobs: compose file not found: <path>` and exits with 1.

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

Every setting is required except `[image_pins]`, and an unknown key is an error. nobs uses only `name`, `title`, `project` and `schema_file`; it checks the others but does not use them.

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
| `Device / target` | the `device` label, or `—` when it is missing. Then ` → ` and the target, when the alert has one. The target is the `peer_address` label, or else the `interface` label. An empty label counts as missing. |
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
| 0 | nobs printed the table, or the empty text |
| 1 | Alertmanager did not answer: `nobs: cannot reach Alertmanager at <address>: <error>` |
| 1 | Alertmanager answered with a status code of 400 or more: `nobs: Alertmanager at <address> answered <status>` |
| 1 | the answer is not a JSON list: `nobs: Alertmanager at <address> answered with a body nobs cannot read: <reason>` |
| 2 | a `--label` without `=`, or a flag nobs does not know |

### load-infrahub, schema load and maintenance

Infrahub is the database where the workshop keeps its devices, interfaces and BGP sessions. Each device, interface or BGP session in Infrahub is a node, and each node has a kind, for example `WorkshopDevice`. A schema file tells Infrahub which kinds of node it holds. A lab variables file lists the nodes to put in. Three commands talk to Infrahub, and none makes a Docker call.

Until nobs has its own Infrahub client, each of the three commands runs a Python script through `uv`. `uv` is a tool that runs a Python script together with the packages the script needs, so `uv` must be on the `PATH`. On the first run, `uv` downloads these packages. This can take a while, and `uv` prints nothing while it works, so the command can seem to hang. nobs writes the script to a temporary file for each call and removes the file after the call. The script's own progress lines and error lines show on the screen as they come.

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
| `maintenance` | `--state`, `--clear` | sets the flag to true or to false; the last one wins, and `--state` is the default |
| `maintenance` | `--kind KIND` | gives the kind of the node; the default is `WorkshopDevice` |
| all three | `--address URL` | gives the address of Infrahub |
| all three | `--token TOKEN` | gives the token: the secret key that lets nobs use Infrahub |

A relative path is taken from the workshop folder, not from the current folder. An absolute path is used as it is.

nobs finds the address and the token in this order: the flag, then the variable, then the default.

| Value | Flag | Variable | Default |
|---|---|---|---|
| the address of Infrahub | `--address URL` | `INFRAHUB_ADDRESS` | `http://localhost:8000` |
| the token | `--token TOKEN` | `INFRAHUB_API_TOKEN` | none |

A variable can be set in the shell or in the `.env` file of the workshop; the shell wins. A flag or a variable that is empty or only spaces counts as not given. nobs gives the address and the token to the script as environment variables, never as arguments. A token you give with `--token` is on the command line of nobs itself, and a list of processes shows it. So keep the token in the `.env` file.

The containers reach Infrahub by the name `infrahub-server`, and your machine cannot. When the address names `infrahub-server`, nobs uses `http://localhost:8000` instead, and before the other lines it prints `INFRAHUB_ADDRESS rewritten to host-reachable http://localhost:8000`. A trailing `/` is dropped without a message.

Before it runs anything, `load-infrahub` checks the token, then the schema file (unless you give `--skip-schema`), then the lab variables file. `schema load` checks the schema file first, then the token. `maintenance` checks the token.

`load-infrahub` prints the rewrite line when there is one, then `Applying schema <path>` and `Schema applied` (both left out with `--skip-schema`), then `Loading <path> into Infrahub at <address>`. The progress lines of the script, for example `device srl1 created`, come next. Then one table and one last line. For example:

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

The table has one line for each kind of node, and `Total` is `Created` plus `Updated`. In the last line, the number is the sum of the `Total` column. Run the command again, and the same nodes show under `Updated` instead of `Created`. When the `WorkshopDevice` line shows 0 created and 0 updated, nobs also prints `nobs: no devices found in <path>: is --lab-vars the right file?` on standard error, the output a program uses for errors and warnings. The exit code stays 0.

`schema load` prints the rewrite line when there is one, then `Applying schema <path>` and `Schema applied`.

The `maintenance` flag is a field of each device in Infrahub, `true` or `false`. `maintenance` prints the rewrite line when there is one. When Infrahub has no node of that kind with that name, nobs then prints `nobs: <kind> <device> not found in Infrahub.`, exits with 1 and tells Sonda nothing. Otherwise nobs saves the flag and prints `<device>.maintenance: <before> → <after>`, where each value is `false` or `true`. Then it posts one event to Sonda, and Sonda sends it on to Loki. Grafana shows the event on its annotation panel; an annotation is a mark on a graph at the time of an event. The event is a log line with these parts:

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
| 1 | `maintenance` found no such node: `nobs: <kind> <device> not found in Infrahub.` |
| 1 | `uv` or the script ended with an error: `nobs: the Infrahub script exited <code>`, after the error lines they printed |
| 1 | the script printed text nobs cannot read: `nobs: the Infrahub script answered with text nobs cannot read: <reason>` |
| 1 | nobs cannot write the script file, or cannot start `uv`: `nobs: cannot run uv: <reason>`; when the file cannot be written, `<reason>` is `cannot write <path>: <error>` |
| 1 | the post to Sonda failed: `nobs: sonda /events post failed: <error>`; the flag stays saved in Infrahub |
| 2 | a flag nobs does not know, a missing `-d`, or a missing `PATH` after `schema load` |

In the post failure line, `<error>` is `cannot reach <address>: <reason>` when Sonda does not answer, or `sonda answered <status>: <body>` when it answers with a status code of 400 or more.

The files `captures/infrahub-1.1.jsonl` and `captures/infrahub-1.11.2.jsonl` hold the HTTP requests the script sent to Infrahub 1.1 and to Infrahub 1.11.2, one JSON object per line. Each call of the script starts with one line that names the call and its arguments. Then each request has one line: the method, the address, the headers, the body, the status and the answer. The value of the token is replaced by `<token>` in them.

## Setup and preflight

`nobs preflight` checks that this machine can run a workshop. It prints one line per check: the name of the check, `ok`, `warn` or `fail`, and a detail. It exits with 1 when any check fails, and with 0 when none does. A `warn` does not change the exit code.

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
