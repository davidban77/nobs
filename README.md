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

`nobs <workshop> --help` lists the commands of a workshop. There are nine. Every one except `setup` runs `docker compose --project-name <project> -f <folder>/docker-compose.yml`, then the words in the second column below. `<project>` is the `project` setting in the workshop's `nobs.toml`, and `<folder>` is the workshop folder. `SERVICES` are service names from the compose file; when you give none, the command acts on all of them.

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

Every setting is required except `[image_pins]`, and an unknown key is an error. nobs uses only `name`, `title` and `project`; it checks the others but does not use them.

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
