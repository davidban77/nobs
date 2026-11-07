#!/usr/bin/env bash
set -u

NOBS=${NOBS:-nobs}
SMOKE_PORTS=${SMOKE_PORTS:-"8085 9005 9006 8686 3001 9090 9093 3000 8000 9997 4200"}
SMOKE_SLEEP=${SMOKE_SLEEP:-5}
SMOKE_ALERT=${SMOKE_ALERT:-BgpSessionNotUp}

say() {
    printf 'smoke: %s: %s\n' "$WS" "$1"
}

join_lines() {
    local out="" line
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        out=${out:+$out, }$line
    done
    printf '%s' "$out"
}

read_settings() {
    PROJECT=""
    NAME=""
    if cd "$ROOT/workshops/$WS" 2>/dev/null; then
        PROJECT=$(grep -m 1 '^project = "' nobs.toml 2>/dev/null | sed 's/^project = "\([^"]*\)".*$/\1/')
        NAME=$(grep -m 1 '^name = "' nobs.toml 2>/dev/null | sed 's/^name = "\([^"]*\)".*$/\1/')
    fi
    if [ -z "$PROJECT" ]; then
        say "no project in nobs.toml; refusing to start"
        return 1
    fi
    if [ -z "$NAME" ]; then
        say "no name in nobs.toml; refusing to start"
        return 1
    fi
}

guard() {
    local noun=$1 listing=$2 out code
    shift 2
    out=$("$@")
    code=$?
    if [ "$code" -ne 0 ]; then
        say "docker $listing failed (exit $code); refusing to start"
        return 1
    fi
    if [ -n "$out" ]; then
        say "$noun of project $PROJECT exist ($(printf '%s\n' "$out" | join_lines)); refusing to start"
        return 1
    fi
}

guards() {
    local label="label=com.docker.compose.project=$PROJECT" busy="" port
    guard containers ps docker ps -a --filter "$label" --format '{{.Names}}' || return 1
    guard volumes "volume ls" docker volume ls --filter "$label" --format '{{.Name}}' || return 1
    guard networks "network ls" docker network ls --filter "$label" --format '{{.Name}}' || return 1
    for port in $SMOKE_PORTS; do
        if (exec 3<>/dev/tcp/127.0.0.1/"$port") 2>/dev/null; then
            busy=${busy:+$busy, }$port
        fi
    done
    if [ -n "$busy" ]; then
        say "ports in use ($busy); refusing to start"
        return 1
    fi
}

run_nobs() {
    local code
    say "$*"
    "$NOBS" "$NAME" "$@"
    code=$?
    if [ "$code" -ne 0 ]; then
        say "$* failed (exit $code)"
    fi
    return "$code"
}

wait_url() {
    local url=$1
    say "wait $url"
    for _ in $(seq 1 60); do
        curl -sfo /dev/null --max-time 3 "$url" && break
        sleep "$SMOKE_SLEEP"
    done
    curl -sfo /dev/null --max-time 3 "$url" \
        || { say "$url never ready"; return 1; }
    say "$url is ready"
}

wait_listener() {
    say "wait Infrahub HTTP listener"
    for _ in $(seq 1 60); do
        curl -sfo /dev/null --max-time 3 http://localhost:8000/health \
            || curl -sfo /dev/null --max-time 3 http://localhost:8000/api/healthcheck \
            && break
        sleep "$SMOKE_SLEEP"
    done
    curl -sfo /dev/null --max-time 3 http://localhost:8000/health \
        || curl -sfo /dev/null --max-time 3 http://localhost:8000/api/healthcheck \
        || { say "Infrahub HTTP listener never came up"; return 1; }
    say "Infrahub listener is up"
}

wait_schema() {
    local code=000 token
    say "wait Infrahub schema writes"
    token=$(sed -n 's/^INFRAHUB_API_TOKEN=//p' ./.env | tail -n 1 | sed -e 's/^"\(.*\)"$/\1/' -e "s/^'\(.*\)'\$/\1/")
    for _ in $(seq 1 60); do
        code=$(
            curl -s -o /dev/null -w "%{http_code}" --max-time 5 \
                -X POST -H "Content-Type: application/json" \
                -H "X-INFRAHUB-KEY: $token" \
                -d '{"schemas":[{"version":"1.0"}]}' \
                "http://localhost:8000/api/schema/load?branch=main" || echo 000
        )
        [ "$code" = "200" ] && break
        sleep "$SMOKE_SLEEP"
    done
    [ "$code" = "200" ] \
        || { say "Infrahub never ready for schema writes (last code: $code)"; return 1; }
    say "Infrahub schema-write path is ready"
}

wait_alert() {
    local out code=0 count
    say "wait $SMOKE_ALERT alert"
    for _ in $(seq 1 24); do
        out=$("$NOBS" "$NAME" alerts -l "alertname=$SMOKE_ALERT")
        code=$?
        count=$(printf '%s\n' "$out" | grep -cw firing)
        if [ "$count" -gt 0 ]; then
            say "$SMOKE_ALERT is firing ($count alert(s))"
            return 0
        fi
        sleep "$SMOKE_SLEEP"
    done
    if [ "$code" -ne 0 ]; then
        say "alerts failed (exit $code)"
    fi
    say "no firing $SMOKE_ALERT alert after 120s"
    return 1
}

steps() {
    run_nobs setup \
        && run_nobs up \
        && wait_url http://localhost:9090/-/ready \
        && wait_url http://localhost:3001/ready \
        && wait_url http://localhost:9093/-/ready \
        && wait_listener \
        && wait_schema \
        && run_nobs load-infrahub \
        && wait_alert \
        && run_nobs try-it --auto
}

dump_logs() {
    local service
    say logs
    for service in prefect-flows webhook alertmanager prometheus loki infrahub-server; do
        printf '===== %s =====\n' "$service"
        "$NOBS" "$NAME" logs --no-follow --tail 100 "$service"
    done
}

destroy_stack() {
    local code
    trap - EXIT
    say destroy
    "$NOBS" "$NAME" destroy
    code=$?
    if [ "$code" -ne 0 ]; then
        say "destroy failed (exit $code)"
    fi
    return "$code"
}

on_signal() {
    exit 1
}

smoke_workshop() {
    local failed=0
    WS=$1
    SECONDS=0
    if read_settings && guards; then
        trap destroy_stack EXIT
        trap on_signal INT TERM
        if ! steps; then
            dump_logs
            failed=1
        fi
        destroy_stack || failed=1
        trap - EXIT INT TERM
    else
        failed=1
    fi
    if [ "$SECONDS" -gt 1500 ]; then
        say "over the 25-minute limit"
        failed=1
    fi
    if [ "$failed" -eq 0 ]; then
        say "ok in ${SECONDS}s"
    else
        say "failed in ${SECONDS}s"
    fi
    return "$failed"
}

if [ $# -lt 1 ]; then
    echo "smoke: usage: scripts/smoke.sh ROOT [WORKSHOP ...]"
    exit 2
fi
ROOT=$1
shift
case $ROOT in
    /*) ;;
    *) ROOT=$(pwd)/$ROOT ;;
esac
case $NOBS in
    /*) ;;
    */*) NOBS=$(pwd)/$NOBS ;;
esac
if [ $# -eq 0 ]; then
    set -- autocon5 packt
fi

total=0
passed=0
for workshop in "$@"; do
    total=$((total + 1))
    if smoke_workshop "$workshop"; then
        passed=$((passed + 1))
    fi
done
printf 'smoke: %s of %s workshops ok\n' "$passed" "$total"
if [ "$passed" -ne "$total" ]; then
    exit 1
fi
