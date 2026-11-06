# /// script
# requires-python = ">=3.11"
# dependencies = ["infrahub-sdk==1.23.2", "pyyaml==6.0.3"]
# ///
"""Fallback Infrahub client for nobs: schema load, workshop seed and the maintenance toggle."""

import argparse
import json
import os
import sys
import time
from pathlib import Path
from typing import Any

import httpx
import yaml
from infrahub_sdk import Config, InfrahubClientSync

KINDS = ["WorkshopDevice", "WorkshopInterface", "WorkshopBgpSession"]
DEFAULT_ADDRESS = "http://localhost:8000"
KEY_HEADER = "x-infrahub-key"
TEXT_LIMIT = 2000


class Failure(Exception):
    pass


def emit(result: dict[str, Any]) -> None:
    print(json.dumps(result))


def progress(line: str) -> None:
    print(line, file=sys.stderr)


def schema(client: Any, file: str) -> None:
    document = yaml.safe_load(Path(file).read_text())
    response = client.schema.load(schemas=[document], branch="main")
    if response.errors:
        raise Failure(json.dumps(response.errors))
    emit({"ok": True})


def wait_for_workshop_schema(client: Any, kinds: list[str], timeout: float, branch: str = "main") -> None:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            client.schema.fetch(branch=branch)
            for kind in kinds:
                client.schema.get(kind=kind, branch=branch)
            return
        except Exception as error:
            last_error = error
            time.sleep(1.0)
    raise Failure(
        f"workshop schema kinds {kinds!r} not visible to Infrahub after "
        f"{timeout:.0f}s ({type(last_error).__name__}: {last_error})."
    )


def upsert_device(client: Any, name: str, node: dict[str, Any]) -> tuple[Any, bool]:
    existing = client.filters(kind="WorkshopDevice", name__value=name)
    if existing:
        device = existing[0]
        device.asn.value = node.get("asn")
        device.maintenance.value = bool(node.get("maintenance", False))
        device.site_name.value = node.get("site", "lab")
        device.role.value = node.get("role", "edge")
        device.save()
        return device, False
    device = client.create(
        kind="WorkshopDevice",
        name=name,
        asn=node.get("asn"),
        maintenance=bool(node.get("maintenance", False)),
        site_name=node.get("site", "lab"),
        role=node.get("role", "edge"),
    )
    device.save()
    return device, True


def interface_ip(intf: dict[str, Any]) -> str | None:
    raw = intf.get("ipv4") or intf.get("ip")
    if isinstance(raw, list):
        return raw[0] if raw else None
    return raw


def upsert_interface(client: Any, device: Any, intf: dict[str, Any]) -> bool:
    name = intf["name"]
    existing = client.filters(kind="WorkshopInterface", name__value=name, device__ids=[device.id])
    payload = {
        "name": name,
        "role": intf.get("role", "peer"),
        "ip_address": interface_ip(intf),
        "expected_state": intf.get("expected_state", "up"),
        "device": device.id,
    }
    if existing:
        node = existing[0]
        for key, value in payload.items():
            if key == "device":
                continue
            getattr(node, key).value = value
        node.save()
        return False
    node = client.create(kind="WorkshopInterface", **payload)
    node.save()
    return True


def upsert_bgp_session(client: Any, device: Any, session: dict[str, Any], afi_safi: str) -> bool:
    peer_address = session["peer_ip"]
    existing = client.filters(
        kind="WorkshopBgpSession",
        peer_address__value=peer_address,
        device__ids=[device.id],
    )
    payload = {
        "peer_address": peer_address,
        "remote_as": session.get("remote_as"),
        "afi_safi": afi_safi,
        "expected_state": session.get("expected_state", "established"),
        "expected_prefixes_received": session.get("expected_prefixes_received"),
        "reason": session.get("reason"),
        "device": device.id,
    }
    if existing:
        node = existing[0]
        for key, value in payload.items():
            if key == "device":
                continue
            getattr(node, key).value = value
        node.save()
        return False
    node = client.create(kind="WorkshopBgpSession", **payload)
    node.save()
    return True


def seed(client: Any, lab_vars: str) -> None:
    counts = {kind: {"created": 0, "updated": 0} for kind in KINDS}
    lab = yaml.safe_load(Path(lab_vars).read_text())
    nodes = lab.get("nodes") or {}
    intent_bgp = (lab.get("observability_intent") or {}).get("bgp") or {}
    afi_safi = intent_bgp.get("afi_safi", "ipv4-unicast")
    intended_peers = intent_bgp.get("intended_peers") or {}

    wait_for_workshop_schema(client, kinds=KINDS, timeout=30.0)

    for device_name, node_def in nodes.items():
        device, was_created = upsert_device(client, device_name, node_def)
        counts["WorkshopDevice"]["created" if was_created else "updated"] += 1
        progress(f"device {device_name} {'created' if was_created else 'updated'}")

        for intf in node_def.get("interfaces", []) or []:
            was_created = upsert_interface(client, device, intf)
            counts["WorkshopInterface"]["created" if was_created else "updated"] += 1

        for session in intended_peers.get(device_name, []) or []:
            was_created = upsert_bgp_session(client, device, session, afi_safi=afi_safi)
            counts["WorkshopBgpSession"]["created" if was_created else "updated"] += 1

    emit(counts)


def maintenance(client: Any, device: str, state: bool, kind: str) -> None:
    matches = client.filters(kind=kind, name__value=device)
    if not matches:
        emit({"found": False})
        return
    node = matches[0]
    previous = bool(node.maintenance.value)
    node.maintenance.value = state
    node.save()
    emit({"found": True, "previous": previous, "state": state})


def install_capture(path: Path, token: str) -> None:
    """Append one JSON line per HTTP request; the token is replaced wherever it would appear."""
    secrets = {token, json.dumps(token)[1:-1]}

    def scrub(text: str) -> str:
        for secret in secrets:
            text = text.replace(secret, "<token>")
        return text

    def append(record: dict[str, Any]) -> None:
        with path.open("a", encoding="utf-8") as file:
            file.write(scrub(json.dumps(record)) + "\n")

    def body(raw: bytes) -> Any:
        text = scrub(raw.decode("utf-8", errors="replace"))
        try:
            return json.loads(text)
        except ValueError:
            return text[:TEXT_LIMIT]

    def headers(request: httpx.Request) -> dict[str, str]:
        found = {}
        for key, value in request.headers.raw:
            name = key.decode("latin-1")
            found[name] = "<token>" if name.lower() == KEY_HEADER else value.decode("latin-1")
        return found

    path.parent.mkdir(parents=True, exist_ok=True)
    append({"call": sys.argv[1:]})
    send = httpx.Client.send

    def capturing_send(self: httpx.Client, request: httpx.Request, **kwargs: Any) -> httpx.Response:
        record: dict[str, Any] = {
            "method": request.method,
            "url": str(request.url),
            "request_headers": headers(request),
            "request_body": body(request.read()),
        }
        try:
            response = send(self, request, **kwargs)
        except Exception as error:
            append({**record, "status": None, "response_body": None, "error": f"{type(error).__name__}: {error}"})
            raise
        try:
            raw = response.content
        except httpx.ResponseNotRead:
            raw = b""
        append({**record, "status": response.status_code, "response_body": body(raw)})
        return response

    httpx.Client.send = capturing_send


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Fallback Infrahub client for nobs.")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("schema", help="load a schema file").add_argument("file")
    commands.add_parser("seed", help="upsert the workshop nodes from a lab_vars file").add_argument("lab_vars")
    toggle = commands.add_parser("maintenance", help="set a device's maintenance flag")
    toggle.add_argument("device")
    toggle.add_argument("state", choices=["true", "false"])
    toggle.add_argument("--kind", default="WorkshopDevice")
    return parser


def run(args: argparse.Namespace) -> None:
    token = os.environ.get("INFRAHUB_API_TOKEN", "")
    if not token:
        raise Failure("INFRAHUB_API_TOKEN is required")
    address = os.environ.get("INFRAHUB_ADDRESS", "").strip() or DEFAULT_ADDRESS
    capture = os.environ.get("NOBS_INFRAHUB_CAPTURE", "").strip()
    if capture:
        install_capture(Path(capture), token)
    client = InfrahubClientSync(address=address, config=Config(api_token=token))
    if args.command == "schema":
        schema(client, args.file)
    elif args.command == "seed":
        seed(client, args.lab_vars)
    else:
        maintenance(client, args.device, args.state == "true", args.kind)


def main() -> int:
    args = build_parser().parse_args()
    try:
        run(args)
    except Exception as error:
        print(f"infrahub: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
