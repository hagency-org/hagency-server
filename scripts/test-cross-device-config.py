#!/usr/bin/env python3
"""Cooperate with test-agent-integration.py's isolated embedded server.

Uses two real Pasion grants and two NativeOwner processes. Provider replies are
an explicit local subprocess fixture; this does not invoke a model or read a
production provider account. All grants stay in owner-only fixture files.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import time
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlparse
from urllib.request import (build_opener, HTTPRedirectHandler,
                            HTTPCookieProcessor, ProxyHandler, Request)
import http.cookiejar


class OAuthRejected(RuntimeError):
    def __init__(self, path, status):
        self.status = status
        super().__init__("cross-device OAuth rejected at " + path.split("?")[0]
                         + " (HTTP " + str(status) + ")")


def grant(base, username, password, device):
    class NoRedirect(HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            return None

    browser = build_opener(ProxyHandler({}), NoRedirect(),
                           HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def call(path, body=None, form=False, extra=None):
        data = None if body is None else (
            urlencode(body).encode() if form else json.dumps(body).encode())
        headers = {"Content-Type": "application/x-www-form-urlencoded"
                   if form else "application/json"}
        headers.update(extra or {})
        try:
            with browser.open(Request(base + path, data=data, headers=headers),
                              timeout=20) as reply:
                return json.load(reply)
        except HTTPError as error:
            if error.code in (302, 303, 307):
                return {"redirect_url": error.headers["Location"]}
            raise OAuthRejected(path, error.code) from None

    call("/auth/api/v1/auth/login", {"username": username, "password": password})
    redirect = "http://127.0.0.1:13377/console/server-login/callback"
    registered = call("/auth/oauth2/registration", {
        "client_name": "Cross-device configuration fixture " + device,
        "client_uri": "https://github.com/chrislearn/hagency-client",
        "application_type": "native", "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "redirect_uris": ["http://127.0.0.1/console/server-login/callback"]})
    verifier = secrets.token_urlsafe(48)
    state = secrets.token_urlsafe(32)
    challenge = base64.urlsafe_b64encode(
        hashlib.sha256(verifier.encode()).digest()).decode().rstrip("=")
    authorization = call("/auth/authorize?" + urlencode({
        "client_id": registered["client_id"], "response_type": "code",
        "redirect_uri": redirect, "state": state,
        "code_challenge": challenge, "code_challenge_method": "S256",
        "scope": "urn:matrix:client:api:* urn:matrix:client:device:" + device}))
    destination = authorization["redirect_url"]
    if urlparse(destination).path != "/console/server-login/callback":
        consent = destination.rstrip("/").rsplit("/", 1)[-1]
        destination = call("/auth/api/v1/oauth2/consent/" + consent,
                           {"action": "consent"}, extra={"Origin": base})["redirect_url"]
    parsed = urlparse(destination)
    query = parse_qs(parsed.query)
    if parsed.netloc != "127.0.0.1:13377" or query.get("state") != [state]:
        raise RuntimeError("cross-device callback identity mismatch")
    token = call("/auth/oauth2/token", {
        "grant_type": "authorization_code", "client_id": registered["client_id"],
        "redirect_uri": redirect, "code": query["code"][0],
        "code_verifier": verifier}, form=True)["access_token"]
    # Fixture bootstrapping queues asynchronous Pasion user/device provisioning.
    # Its first job can run before the embedded Palpo listener and retry after
    # five seconds. Require actual authenticated Matrix readiness, never invent
    # an identity or bypass this check. Permanent rejection still fails.
    deadline = time.monotonic() + 30
    waiting = False
    while True:
        try:
            identity = call("/_matrix/client/v3/account/whoami", extra={
                "Authorization": "Bearer " + token})
            break
        except OAuthRejected as error:
            if error.status not in (401, 404, 500, 503) or time.monotonic() >= deadline:
                raise
            if not waiting:
                print("Waiting for fixture Matrix user/device provisioning (30s limit).",
                      flush=True)
                waiting = True
            time.sleep(0.5)
    return {"accessToken": token, "clientId": registered["client_id"],
            "owner": identity["user_id"]}


def main():
    path = Path(sys.argv[1]).resolve()
    if path.stat().st_mode & 0o077:
        raise RuntimeError("fixture is not private")
    fixture = json.loads(path.read_text())
    origin = urlparse(fixture["base"])
    if (origin.scheme != "http" or origin.hostname != "127.0.0.1"
            or origin.port is None or origin.path or origin.query or origin.fragment
            or origin.username is not None or origin.password is not None):
        raise RuntimeError("cross-device fixture must use the isolated loopback server")
    first = grant(fixture["base"], fixture["username"], fixture["password"], "CrossConfigA")
    second = grant(fixture["base"], fixture["username"], fixture["password"], "CrossConfigB")
    if first["owner"] != second["owner"]:
        raise RuntimeError("fixture owner changed")
    output = path.parent / "cross-device-fixture.json"
    with os.fdopen(os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as file:
        json.dump({"base": fixture["base"], "a": first, "b": second}, file)
    env = os.environ.copy()
    env["HAGENCY_REAL_CROSS_DEVICE_FIXTURE"] = str(output)
    client_root = Path(__file__).resolve().parents[2] / "hagency-client"
    subprocess.run(["cargo", "test", "--locked", "-p", "hagency", "--test",
                    "cross_device_server", "real_two_device_configuration", "--",
                    "--ignored", "--exact", "--test-threads=1"],
                   cwd=client_root, env=env, check=True)


if __name__ == "__main__":
    main()
