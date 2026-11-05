# nobs

A terminal program (a text user interface) for network labs and observability workshops. It replaces the labby command line tool and takes in the lab tooling of labcli, the workshops and the network observability lab.

Status: early. The `nobs docker` group (13 commands), the `nobs containerlab` group (3 commands) and the `nobs sonda` group (10 commands) work. The planning papers (requirements, findings, decisions and the plan) are not kept in this repository.

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

Run the `containerlab` commands inside the Linux machine `clab`, with the Linux binary: `orb -m clab <path> containerlab deploy`, where `<path>` is the full path of `target/aarch64-unknown-linux-musl/release/nobs`. nobs runs containerlab with `sudo` unless you give `--no-sudo`. By default, sudo does not pass on the variables of your shell. So nobs reads the topology file, finds each variable it uses as `${NAME}`, and tells sudo to keep it. Set such a variable inside `clab`, for example `orb -m clab env NAME=value <path> containerlab deploy`, or in the `.env` file of the current folder.

## Sonda

Sonda makes test metrics and logs, and `sonda-server` is its HTTP server. The `nobs sonda` commands talk to a running `sonda-server`. Each command is one HTTP call, and nobs prints what the server answers.

| Command | HTTP call | What it prints |
|---|---|---|
| `nobs sonda health` | `GET /health` | the reply |
| `nobs sonda ready` | `GET /ready` | the reply |
| `nobs sonda list` | `GET /scenarios` | a table with the columns `ID`, `NAME`, `STATE`, `ELAPSED` and `DEGRADED`, and a header only when there are no scenarios |
| `nobs sonda show ID` | `GET /scenarios/ID` | the reply |
| `nobs sonda stats ID` | `GET /scenarios/ID/stats` | the reply |
| `nobs sonda delete ID` | `DELETE /scenarios/ID` | the reply |
| `nobs sonda load FILE` | `POST /scenarios` | the reply |
| `nobs sonda event FILE` | `POST /events` | the reply |
| `nobs sonda metrics` | `GET /metrics` | the metrics text as the server sent it |
| `nobs sonda version` | `GET /metrics` | `sonda-server <version> <git_sha>`, read from the `sonda_server_build_info` line |

The server address comes from `--server URL`, then the variable `SONDA_SERVER_URL` in the shell, then the same variable in the `.env` file of the current folder, then `http://localhost:8080`. The key comes from `--api-key KEY`, then `SONDA_API_KEY` in the same way. With a key, every call except `health` and `ready` sends it as `Authorization: Bearer <key>`. An empty value counts as not set, so nobs moves to the next place. nobs never writes the key anywhere. Both flags can go before or after the command, for example `nobs sonda --server http://127.0.0.1:8085 list`.

nobs talks plain HTTP only, so an address that starts with `https://` does not work yet.

When the reply is JSON (a text format for data), nobs prints it with one field per line and the fields sorted by name.

`nobs sonda load FILE` sends a file only when it sees `kind: runnable` at the start of a line, at the top level. It refuses any other file with the reason, exits with 1, and sends nothing.

When the server answers with a status of 400 or more, nobs prints `nobs: sonda answered <status>: <body>` and exits with 1.
