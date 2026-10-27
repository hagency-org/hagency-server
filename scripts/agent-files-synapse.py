#!/usr/bin/env python3
"""Start one private, non-federating Synapse qualification instance.

Only explicit new directories and loopback published ports are accepted. The
caller retains its data and stops the named container after qualification.
This is not a production installer or Pasion OAuth qualification.
"""
import argparse
import json
import ipaddress
import os
from pathlib import Path
import secrets
import subprocess
import time
import urllib.error
import urllib.request

IMAGE = "ghcr.io/element-hq/synapse@sha256:c3c21919a322f74d177a43bce8031adc81ef839ddd117a8dd03cfec350e76e02"


def request(origin, path, body=None, token=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    req = urllib.request.Request(origin + path, headers=headers,
                                 data=None if body is None else json.dumps(body).encode())
    with urllib.request.urlopen(req, timeout=5) as response:
        return json.load(response)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--root", type=Path, required=True)
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--agent-port", type=int, required=True)
    p.add_argument("--retain-owner-password", action="store_true",
                   help="Keep only this new isolated owner's password privately for multi-device SDK qualification")
    p.add_argument("--callback-host", default="host.docker.internal",
                   help="Docker host callback; use the Docker internal IPv4 if IPv6 is unavailable")
    a = p.parse_args()
    if a.root.exists() or not all(1024 <= n <= 65535 for n in [a.port, a.agent_port]):
        p.error("a new private directory and unprivileged explicit ports are required")
    if a.callback_host != "host.docker.internal":
        address = ipaddress.ip_address(a.callback_host)
        if address.version != 4 or not address.is_private:
            p.error("callback host must be the Docker internal private IPv4")
    os.umask(0o077)
    a.root.mkdir(parents=True, mode=0o700)
    a.root = a.root.resolve()
    data = a.root / "synapse"
    data.mkdir(mode=0o700)
    origin = f"http://127.0.0.1:{a.port}"
    name = "hagency-files-synapse-" + secrets.token_hex(5)
    as_token = "fixture-joint-as-token-only-not-real-secret"
    hs_token = "fixture-joint-hs-token-only-not-real-secret"
    (data / "appservice.yaml").write_text(json.dumps({
        "id": "hagency-files-joint", "url": f"http://{a.callback_host}:{a.agent_port}",
        "as_token": as_token, "hs_token": hs_token, "sender_localpart": "_hagency_service",
        "namespaces": {"users": [{"regex": "@_hagency_.*:test", "exclusive": True}]},
        "rate_limited": False,
    }))
    (data / "homeserver.yaml").write_text(json.dumps({
        "server_name": "test", "public_baseurl": origin + "/", "report_stats": False,
        "pid_file": "/data/synapse.pid", "signing_key_path": "/data/test.signing.key",
        "macaroon_secret_key": secrets.token_hex(32), "form_secret": secrets.token_hex(32),
        "listeners": [{"port": 8008, "type": "http", "tls": False,
                       "bind_addresses": ["0.0.0.0"],
                       "resources": [{"names": ["client"], "compress": False}]}],
        "database": {"name": "sqlite3", "args": {"database": "/data/homeserver.db"}},
        "media_store_path": "/data/media_store", "max_upload_size": "10M",
        "enable_registration": True, "enable_registration_without_verification": True,
        "enable_authenticated_media": True, "suppress_key_server_warning": True,
        "trusted_key_servers": [], "federation_domain_whitelist": [],
        "app_service_config_files": ["/data/appservice.yaml"],
        "rc_registration": {"per_second": 100, "burst_count": 100},
        "rc_message": {"per_second": 100, "burst_count": 100},
        "rc_admin_redaction": {"per_second": 100, "burst_count": 100},
    }))
    subprocess.run(["docker", "run", "-d", "--name", name, "--user", "0:0",
                    "--label", "org.hagency.qualification=agent-files",
                    "--mount", f"type=bind,source={data},target=/data",
                    "-p", f"127.0.0.1:{a.port}:8008", IMAGE], check=True,
                   stdout=subprocess.DEVNULL)
    (a.root / "container-name.txt").write_text(name + "\n")
    deadline = time.monotonic() + 45
    while True:
        try:
            request(origin, "/_matrix/client/versions")
            break
        except (OSError, urllib.error.URLError):
            if time.monotonic() >= deadline:
                raise RuntimeError("original Synapse did not become ready; inspect retained container logs")
            time.sleep(0.2)
    users = {}
    for user in ["alice", "bob"]:
        password = secrets.token_urlsafe(30)
        session = request(origin, "/_matrix/client/v3/register", {
            "username": user, "password": password, "auth": {"type": "m.login.dummy"},
            "device_id": "JOINT-SDK-" + user.upper(),
        })
        users[user] = session
        if user == "alice" and a.retain_owner_password:
            (a.root / "owner-password.txt").write_text(password)
    request(origin, "/_matrix/client/v3/register?kind=user", {
        "type": "m.login.application_service", "username": "_hagency_service",
        "inhibit_login": True,
    }, as_token)
    (a.root / "synapse-bootstrap.json").write_text(json.dumps({
        "matrixOrigin": origin + "/", "owner": users["alice"],
        "requester": users["bob"], "container": name, "image": IMAGE,
    }, indent=2))
    print("Synapse ready; private bootstrap and container identity retained in", a.root)


if __name__ == "__main__":
    main()
