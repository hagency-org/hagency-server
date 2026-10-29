#!/usr/bin/env python3
"""Real embedded Palpo/Pasion smoke test using three disposable databases.

Never uses the application databases. Secrets stay in private fixture files and
memory. Does not invoke a model or tool runner, and does not modify Palpo source.
HAGENCY_TEST_BACKUP_RESTORE=1 enables the full native PKCE flow plus stopped
three-database restoration into separate disposable databases.
"""
import json
import contextlib
import os
from pathlib import Path
import re
import secrets
import socket
import shutil
import subprocess
import sys
import tempfile
import time
from urllib.parse import quote, urlencode, urlparse, parse_qs
from urllib.request import Request, urlopen, build_opener, ProxyHandler, HTTPRedirectHandler, HTTPCookieProcessor
import http.cookiejar
import hashlib
import base64
from urllib.error import HTTPError, URLError
import uuid

ROOT = Path(__file__).resolve().parent.parent
CONTAINER = os.environ.get("HAGENCY_TEST_POSTGRES_CONTAINER", "hagency-server-postgres-1")
PORT = os.environ.get("HAGENCY_TEST_POSTGRES_PORT", "55438")
USER = subprocess.check_output(["docker", "exec", CONTAINER, "printenv", "POSTGRES_USER"], text=True).strip()
PASSWORD = subprocess.check_output(["docker", "exec", CONTAINER, "printenv", "POSTGRES_PASSWORD"], text=True).strip()
PREFIX = "hagency_smoke_" + uuid.uuid4().hex[:12]
DBS = [PREFIX + suffix for suffix in ["_agent", "_matrix", "_auth"]]
CREATED = []
SERVER = None
SUCCESS = False
FOLDER = None
FIXTURE_FOLDERS = []
BACKUP_RESTORE = os.environ.get("HAGENCY_TEST_BACKUP_RESTORE") == "1"
NATIVE_PKCE = BACKUP_RESTORE or os.environ.get("HAGENCY_TEST_NATIVE_PKCE") == "1"

def entity_id(value, prefix):
    assert re.fullmatch(re.escape(prefix) + r"[0-7][0-9abcdefghjkmnpqrstvwxyz]{25}", value), "invalid lowercase ULID entity ID"

def sql(database, statement, capture=False):
    result = subprocess.run(["docker", "exec", CONTAINER, "psql", "-U", USER,
        "-d", database, "-v", "ON_ERROR_STOP=1", "-At", "-c", statement],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if result.returncode:
        raise RuntimeError("isolated database operation failed")
    return result.stdout.strip() if capture else None

def dburl(database):
    return "postgres://" + quote(USER, safe="") + ":" + quote(PASSWORD, safe="") + "@127.0.0.1:" + PORT + "/" + database

def private(path, body):
    fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(body)

def request(origin, method, path, body=None, token=None, expected_status=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    data = None if body is None else json.dumps(body).encode()
    try:
        with urlopen(Request(origin + path, data=data, headers=headers, method=method), timeout=15) as response:
            if expected_status is not None:
                assert response.status == expected_status, "unexpected success status"
            return json.load(response)
    except HTTPError as e:
        if expected_status is not None:
            assert e.code == expected_status, "unexpected rejection status: " + str(e.code)
            return json.load(e)
        try:
            code = json.load(e).get("code", "request_failed")
        except Exception:
            code = "request_failed"
        raise RuntimeError("HTTP " + str(e.code) + " at " + path.split("?")[0] + ": " + str(code)) from None

def native_oauth(origin, password):
    class NoRedirect(HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            return None
    opener=build_opener(ProxyHandler({}),NoRedirect(),HTTPCookieProcessor(http.cookiejar.CookieJar()))
    def call(path, body=None, form=False, extra=None):
        data=None if body is None else (urlencode(body).encode() if form else json.dumps(body).encode())
        headers={"Content-Type":"application/x-www-form-urlencoded" if form else "application/json"}
        headers.update(extra or {})
        try:
            with opener.open(Request(origin+path,data=data,headers=headers),timeout=15) as reply:
                return json.load(reply)
        except HTTPError as error:
            if error.code in [302,303,307]:
                return {"redirect_url":error.headers["Location"]}
            raise RuntimeError("native PKCE protocol step failed at "+path.split("?")[0]) from None
    call("/auth/api/v1/auth/login",{"username":"agent_smoke_owner","password":password})
    redirect="http://127.0.0.1:13377/console/server-login/callback"
    registered=call("/auth/oauth2/registration",{"client_name":"Isolated native PKCE smoke","client_uri":"https://github.com/chrislearn/hagency-client","application_type":"native","token_endpoint_auth_method":"none","grant_types":["authorization_code","refresh_token"],"response_types":["code"],"redirect_uris":["http://127.0.0.1/console/server-login/callback"]})
    verifier=secrets.token_urlsafe(48);state=secrets.token_urlsafe(32)
    challenge=base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip("=")
    authorization=call("/auth/authorize?"+urlencode({"client_id":registered["client_id"],"response_type":"code","redirect_uri":redirect,"state":state,"code_challenge":challenge,"code_challenge_method":"S256","scope":"urn:matrix:client:api:* urn:matrix:client:device:HagencySmoke"}))
    destination=authorization["redirect_url"]
    if urlparse(destination).path!="/console/server-login/callback":
        consent=destination.rstrip("/").rsplit("/",1)[-1]
        destination=call("/auth/api/v1/oauth2/consent/"+consent,{"action":"consent"},extra={"Origin":origin})["redirect_url"]
    parsed=urlparse(destination);query=parse_qs(parsed.query)
    assert parsed.scheme=="http" and parsed.netloc=="127.0.0.1:13377" and query["state"]==[state]
    return call("/auth/oauth2/token",{"grant_type":"authorization_code","client_id":registered["client_id"],"redirect_uri":redirect,"code":query["code"][0],"code_verifier":verifier},form=True)["access_token"]

def browser_api_paths(origin, password):
    """Exercise the migrated browser BFF against the real embedded server."""
    token = native_oauth(origin, password)
    cookies = http.cookiejar.CookieJar()
    opener = build_opener(ProxyHandler({}), HTTPCookieProcessor(cookies))

    csrf = None
    def call(path, method="GET", body=None, expected=200, extra=None):
        headers = {"Content-Type": "application/json", "Origin": origin}
        if method == "GET" and csrf:
            headers["x-csrf-token"] = csrf
        headers.update(extra or {})
        data = None if body is None else json.dumps(body).encode()
        req = Request(origin + path, data=data, headers=headers, method=method)
        try:
            reply = opener.open(req, timeout=15)
        except HTTPError as error:
            reply = error
        with reply:
            content_type = reply.headers.get_content_type()
            payload = json.load(reply) if content_type == "application/json" else None
            code = payload.get("code") if isinstance(payload, dict) else None
            code = code if isinstance(code, str) and re.fullmatch(r"[a-z_]{1,64}", code) else "unavailable"
            assert reply.code == expected, (
                "browser API status mismatch at " + path + ": expected " + str(expected)
                + ", got " + str(reply.code) + ", code=" + code)
            assert content_type == "application/json", "API returned SPA HTML"
            return payload

    session = call("/hagency/api/login/token", "POST", {},
                   extra={"Authorization": "Bearer " + token})
    csrf = session["csrf"]
    assert session["userId"].startswith("@agent_smoke_owner:")
    assert any(cookie.name == "palpo_admin" and "HttpOnly" in cookie._rest for cookie in cookies)
    assert call("/hagency/api/session")["userId"] == session["userId"]
    assert isinstance(call("/hagency/api/browser/v1/agents")["agents"], list)
    assert call("/hagency/api/browser/v1/agents", expected=403,
                extra={"x-csrf-token": ""})["code"] == "csrf_forbidden"
    assert call("/hagency/api/browser/v1/agents", "POST", {}, 403)["code"] == "csrf_forbidden"
    assert call("/hagency/api/browser/v1/unknown", expected=404)["code"] == "not_found"
    for path in ["/api/login/token", "/api/session", "/api/logout", "/api/browser/hagency/v1/agents"]:
        call(path, expected=404)
        call(path, "POST", {}, 404)
    call("/hagency/api/logout", "POST", {}, extra={"x-csrf-token": session["csrf"]})
    call("/hagency/api/session", expected=401)
    print("PASS: migrated browser login/session/Agent bridge/CSRF/logout and retired URLs")


try:
    with contextlib.nullcontext(tempfile.mkdtemp(prefix="hagency-agent-integration-")) as temporary:
        folder = Path(temporary)
        FOLDER = folder
        FIXTURE_FOLDERS.append(folder)
        for database in DBS:
            sql("postgres", "CREATE DATABASE " + database)
            CREATED.append(database)
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            listen_port = s.getsockname()[1]
        origin = "http://127.0.0.1:" + str(listen_port)
        server_name = "localhost:" + str(listen_port)
        palpo = (ROOT / "config/dev/palpo.toml").read_text()
        palpo = re.sub(r'^server_name\s*=.*$', 'server_name = ' + json.dumps(server_name), palpo, flags=re.M)
        palpo = re.sub(r'^url\s*=.*$', 'url = ' + json.dumps(dburl(DBS[1])), palpo, flags=re.M)
        palpo = re.sub(r'^client\s*=.*$', 'client = ' + json.dumps(origin), palpo, flags=re.M)
        palpo = re.sub(r'^server\s*=.*$', 'server = ' + json.dumps(server_name), palpo, flags=re.M)
        palpo = re.sub(r'^root\s*=.*$', 'root = ' + json.dumps(str(folder / "media")), palpo, flags=re.M)
        pasion = (ROOT / "config/dev/pasion.toml").read_text()
        pasion = re.sub(r'^uri\s*=.*$', 'uri = ' + json.dumps(dburl(DBS[2])), pasion, flags=re.M)
        pasion = re.sub(r'^resources_dir\s*=.*$', 'resources_dir = ' + json.dumps(os.environ.get("HAGENCY_TEST_PASION_RESOURCES", str(ROOT / "resources/pasion"))), pasion, flags=re.M)
        if BACKUP_RESTORE:
            private(folder / "password-pepper", secrets.token_urlsafe(32))
            pasion += '\n[[passwords.schemes]]\nversion = 1\nalgorithm = "argon2id"\nsecret_file = "password-pepper"\n'
        host = '\n'.join([
            'listen = ' + json.dumps("127.0.0.1:" + str(listen_port)),
            'public_origin = ' + json.dumps(origin),
            'database_url = ' + json.dumps(dburl(DBS[0])),
            'data_dir = ' + json.dumps(str(folder / "data")),
            'palpo_config = "palpo.toml"', 'pasion_config = "pasion.toml"',
            'public_dir = ' + json.dumps(os.environ.get("HAGENCY_TEST_FRONTEND_RESOURCES", str(ROOT / "resources/frontend/public"))), ""])
        private(folder / "palpo.toml", palpo)
        private(folder / "pasion.toml", pasion)
        private(folder / "hagency.toml", host)
        private(folder / "bootstrap-password", secrets.token_urlsafe(32))
        binary = Path(os.environ.get("HAGENCY_TEST_SERVER_BINARY", str(ROOT / "target/debug/hagency-server")))
        with open(folder / "server.log", "wb") as log:
            SERVER = subprocess.Popen([str(binary), "--config", str(folder / "hagency.toml"),
                "--bootstrap-admin", "agent_smoke_owner", "--bootstrap-password-file", str(folder / "bootstrap-password")],
                cwd=ROOT, stdout=log, stderr=log)
            for _ in range(120):
                if SERVER.poll() is not None:
                    raise RuntimeError("isolated embedded server failed to start; logs were kept private")
                try:
                    discovery = request(origin, "GET", "/hagency/api/v1/discovery")
                    assert discovery["protocolVersion"] == 3
                    assert discovery["serverName"] == server_name
                    assert discovery["issuer"] == origin + "/auth/"
                    assert discovery["authorizationEndpoint"] == origin + "/auth/authorize"
                    assert discovery["tokenEndpoint"] == origin + "/auth/oauth2/token"
                    break
                except (URLError, TimeoutError):
                    time.sleep(1)
            else:
                raise RuntimeError("isolated embedded server startup timeout")
            for _ in range(60):
                if request(origin,"GET","/hagency/api/v1/readiness")["startupRoundtripConfirmed"]:
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError("production startup roundtrip gate did not become ready")
            print("PASS: production startup gate verified its own Matrix event in durable AS inbox")
            homepage = urlopen(origin + "/").read().decode()
            assert '<title>Hagency Server</title>' in homepage
            console = urlopen(origin + "/hagency/home").read().decode()
            assert '<title>Hagency Server</title>' in console and console == homepage
            assert urlopen(origin + "/login").read().decode() == homepage
            assert urlopen(origin + "/hagency/projects").read().decode() == console
            print("PASS: public sign-in and authenticated console routes share the bundled frontend")
            for legacy in ["/api/fleets", "/api/approvals", "/api/accounts", "/api/registrations",
                           "/api/hagency/v1/discovery", "/api/browser/hagency/v1/agents",
                           "/api/login/token", "/api/session", "/api/logout",
                           "/hagency/api/unknown"]:
                try:
                    request(origin,"GET",legacy)
                except RuntimeError as e:
                    assert str(e).startswith("HTTP 404 at "+legacy+":"), "legacy API was not removed"
                else:
                    raise RuntimeError("legacy API is still reachable")
            print("PASS: retired API namespaces and unknown Hagency API paths return 404, never SPA HTML")
            registration = json.loads((folder / "data/agent-appservice.json").read_text())
            service = "@_hagency_service:" + server_name
            query = "?user_id=" + quote(service, safe="")
            who = request(origin, "GET", "/_matrix/client/v3/account/whoami" + query, token=registration["as_token"])
            assert who["user_id"] == service
            room=sql(DBS[0],"SELECT room_id FROM hagency_agent_v1.readiness_room WHERE singleton",True)
            assert request(origin,"GET","/_matrix/client/v3/rooms/"+quote(room,safe="")+"/state/m.room.join_rules"+query,token=registration["as_token"])["join_rule"]=="invite"
            path = "/_matrix/client/v3/rooms/" + quote(room, safe="") + "/send/m.room.message/agent_probe" + query
            content = {"msgtype": "m.notice", "body": "Hagency service readiness probe"}
            event = request(origin, "PUT", path, content, registration["as_token"])["event_id"]
            assert request(origin, "PUT", path, content, registration["as_token"])["event_id"] == event
            probe = json.dumps({"events": [{"event_id": event}]}).replace("'", "''")
            for _ in range(30):
                seen = sql(DBS[0], "SELECT count(*) FROM hagency_agent_v1.readiness_receipts WHERE event_id='"+event.replace("'","''")+"'", True)
                if int(seen) > 0:
                    break
                time.sleep(1)
            else:
                raise RuntimeError("real Matrix event did not reach durable Appservice inbox")
            print("PASS: embedded Pasion/Palpo startup, mandatory AS identity, private room, stable Matrix send and durable AS roundtrip")
            if os.environ.get("HAGENCY_TEST_HOLD_BROWSER") == "1":
                private(folder / "browser-fixture.json", json.dumps({"base": origin, "username": "agent_smoke_owner", "password": (folder / "bootstrap-password").read_text()}))
                print("BROWSER fixture ready: " + origin + " ; private directory: " + str(folder), flush=True)
                for _ in range(600):
                    done = folder / "browser-done.json"
                    if done.exists():
                        assert done.stat().st_mode & 0o077 == 0
                        assert json.loads(done.read_text()).get("passed") is True
                        print("PASS: real browser navigation and OAuth authorization", flush=True)
                        break
                    time.sleep(1)
                else:
                    raise RuntimeError("browser verification deadline exceeded")
            if os.environ.get("HAGENCY_TEST_HOLD_CLIENT") == "1":
                private(folder / "client-fixture.json", json.dumps({"base":origin,"username":"agent_smoke_owner","password":(folder/"bootstrap-password").read_text()}))
                print("CLIENT fixture ready: " + origin + " ; private directory: " + str(folder), flush=True)
                if os.environ.get("HAGENCY_TEST_CROSS_DEVICE") == "1":
                    subprocess.run([sys.executable, str(ROOT / "scripts/test-cross-device-config.py"),
                                    str(folder / "client-fixture.json")], check=True)
                for _ in range(600):
                    done = folder / "client-done.json"
                    if done.exists():
                        assert done.stat().st_mode & 0o077 == 0
                        assert json.loads(done.read_text()).get("passed") is True, "native OwnerHost verification failed"
                        print("PASS: cooperating native OwnerHost creation/discovery/Agent verification completed", flush=True)
                        SUCCESS = True
                        sys.exit(0)
                    if SERVER.poll() is not None:
                        raise RuntimeError("client fixture server stopped")
                    time.sleep(1)
                raise RuntimeError("native OwnerHost tester did not complete")
            if os.environ.get("HAGENCY_TEST_HOLD_PKCE") != "1" and not NATIVE_PKCE:
                # This optional compatibility probe is separate from native OAuth.
                # Native PKCE must not depend on legacy Matrix password login.
                human = request(origin, "POST", "/_matrix/client/v3/login", {
                    "type": "m.login.password", "identifier": {"type": "m.id.user", "user": "agent_smoke_owner"},
                    "password": (folder / "bootstrap-password").read_text()})
                matrix_token = human["access_token"]
                try:
                    request(origin, "POST", "/hagency/api/v1/sessions/pasion", {"accessToken": matrix_token})
                except RuntimeError as e:
                    assert str(e).startswith("HTTP 401 at /hagency/api/v1/sessions/pasion:"), "unexpected compatibility proof failure"
                else:
                    raise RuntimeError("compatibility password token incorrectly established native OAuth authority")
                print("PASS: compatibility password token cannot bypass native Pasion OAuth identity requirements")
                SUCCESS = True
                sys.exit(0)
            if os.environ.get("HAGENCY_TEST_HOLD_PKCE") == "1":
                # A cooperating real browser/PKCE tester writes its native OAuth
                # grant into this private fixture, never into tool output.
                print("PKCE fixture ready: " + origin + " ; private directory: " + str(folder), flush=True)
                for _ in range(600):
                    grant_path = folder / "oauth-grant.json"
                    if grant_path.exists():
                        assert grant_path.stat().st_mode & 0o077 == 0
                        matrix_token = json.loads(grant_path.read_text())["access_token"]
                        break
                    if SERVER.poll() is not None:
                        raise RuntimeError("PKCE fixture server stopped")
                    time.sleep(1)
                else:
                    raise RuntimeError("real native PKCE tester did not supply a grant")
            if NATIVE_PKCE:
                browser_api_paths(origin, (folder / "bootstrap-password").read_text())
                matrix_token=native_oauth(origin,(folder/"bootstrap-password").read_text())
                print("PASS: real Pasion native DCR + PKCE + consent protocol grant")
            human = request(origin, "GET", "/_matrix/client/v3/account/whoami", token=matrix_token)
            session = request(origin, "POST", "/hagency/api/v1/sessions/pasion", {"accessToken": matrix_token})
            user_token = session["token"]
            space = request(origin, "POST", "/_matrix/client/v3/createRoom", {
                "preset": "private_chat", "name": "Agent smoke Space", "creation_content": {"type": "m.space"}}, matrix_token)["room_id"]
            discussion = request(origin, "POST", "/_matrix/client/v3/createRoom", {
                "preset": "private_chat", "name": "Agent smoke discussion", "invite": [service]}, matrix_token)["room_id"]
            request(origin, "PUT", "/_matrix/client/v3/rooms/" + quote(space, safe="") + "/state/m.space.child/" + quote(discussion, safe=""), {"via": [server_name]}, matrix_token)
            project = request(origin, "POST", "/hagency/api/v1/projects/adopt", {"spaceId": space}, user_token)["project"]
            request(origin, "POST", "/hagency/api/v1/projects/" + project["id"] + "/rooms/adopt", {"roomId": discussion}, user_token)
            device = request(origin, "POST", "/hagency/api/v1/devices", {"installationId": "smoke_device", "name": "disposable integration device"}, user_token)
            request(origin, "POST", "/hagency/api/v1/sessions/current/renew", {"accessToken": matrix_token}, user_token)
            device_token = device["token"]
            create_body = {"displayName": "Smoke Agent", "idempotencyKey": "smoke_create", "visibility": "public"}
            assert request(origin,"GET","/hagency/api/v1/commands/agent.create/smoke_create",token=user_token,expected_status=404)["code"]=="command_not_found"
            creation = request(origin, "POST", "/hagency/api/v1/agents", create_body, device_token)
            for _ in range(60):
                if creation["commandState"] == "active": break
                time.sleep(0.5)
                creation = request(origin, "POST", "/hagency/api/v1/agents", create_body, device_token)
            assert creation["commandState"] == "active", "global puppet identity provisioning stayed pending"
            agent = creation["creation"]["agent"]
            entity_id(agent["id"], "agt_")
            entity_id(project["id"], "prj_")
            entity_id(session["userId"], "usr_")
            assert agent["puppetMxid"] == "@_hagency_" + agent["id"] + ":" + human["user_id"].split(":", 1)[1]
            assert "binding" not in creation["creation"] and agent["ownerUserId"] == session["userId"]
            bind_path = "/hagency/api/v1/agents/"+agent["id"]+"/bindings"
            bind_body = {"projectId": project["id"], "roomId": discussion, "idempotencyKey": "smoke_bind"}
            project_path = "/hagency/api/v1/projects/" + project["id"]
            room_path = project_path + "/rooms/" + quote(discussion, safe="")
            assert not request(origin,"GET",project_path+"/service-state",token=user_token)["servicePaused"]
            room_state=request(origin,"GET",room_path+"/service-state",token=user_token)
            assert room_state["roomId"]==discussion and not room_state["servicePaused"]
            def denied_creation(key, code):
                body=dict(bind_body,idempotencyKey=key)
                rejection=request(origin,"POST",bind_path,body,user_token,expected_status=401)
                assert rejection["code"]==code, "unexpected authorization rejection"
                assert request(origin,"GET",bind_path,token=user_token)["bindings"]==[], "denied binding allocated a scope"
                assert request(origin,"GET","/hagency/api/v1/commands/agent.bind/"+key,token=user_token,expected_status=404)["code"]=="command_not_found"
            # Persisted scope pauses must block creation even with zero bindings.
            paused=request(origin,"POST",project_path+"/pause-service",token=user_token)
            assert paused["affectedBindings"]==0
            denied_creation("blocked_project_pause","administrator_pause_active")
            request(origin,"POST",project_path+"/clear-service-pause",token=user_token)
            disabled=request(origin,"PUT",room_path+"/creation-policy",{"expectedRevision":room_state["revision"],"policy":{"mode":"disabled"}},user_token)["room"]
            denied_creation("blocked_room_policy","agent_creation_denied")
            restored=request(origin,"PUT",room_path+"/creation-policy",{"expectedRevision":disabled["revision"],"policy":{"mode":"inherit_project","deny":[]}},user_token)["room"]
            assert restored["revision"]>disabled["revision"]
            request(origin,"POST",room_path+"/pause-service",token=user_token)
            denied_creation("blocked_room_pause","administrator_pause_active")
            request(origin,"POST",project_path+"/pause-service",token=user_token)
            merged=request(origin,"GET",room_path+"/service-state",token=user_token)
            assert merged["projectPaused"] and merged["roomPaused"] and merged["servicePaused"]
            request(origin,"POST",room_path+"/clear-service-pause",token=user_token)
            merged=request(origin,"GET",room_path+"/service-state",token=user_token)
            assert merged["projectPaused"] and not merged["roomPaused"] and merged["servicePaused"]
            denied_creation("blocked_remaining_project_pause","administrator_pause_active")
            request(origin,"POST",project_path+"/clear-service-pause",token=user_token)
            assert not request(origin,"GET",room_path+"/service-state",token=user_token)["servicePaused"]
            print("PASS: zero-binding Project/Room pauses and Room disabled policy reject bindings without allocating commands; global Agent identity remains independent; encoded Room policy and merged pause state verified")
            bound_first = request(origin, "POST", bind_path, bind_body, user_token)
            for _ in range(60):
                if bound_first["commandState"] == "active": break
                time.sleep(0.5)
                bound_first = request(origin, "POST", bind_path, bind_body, user_token)
            assert bound_first["commandState"] == "active", "independent Room binding stayed pending"
            binding = bound_first["creation"]["binding"]
            assert binding["scopeKind"] == "project" and binding["projectId"] == project["id"]
            repeat = request(origin, "POST", "/hagency/api/v1/agents", create_body, device_token)
            assert repeat["creation"]["agent"]["id"] == agent["id"]
            status=request(origin,"GET","/hagency/api/v1/commands/agent.create/smoke_create",token=user_token)
            assert status["commandState"]=="active" and status["creation"]["agent"]["id"]==agent["id"] and "binding" not in status["creation"]
            changed=dict(create_body,displayName="Changed replay must fail")
            request(origin,"POST","/hagency/api/v1/agents",changed,device_token,expected_status=409)
            # No bearerless command discovery.
            request(origin,"GET","/hagency/api/v1/commands/agent.create/smoke_create",expected_status=401)
            print("PASS: owner command-status and stable same-key replay; changed payload conflicts and unauthenticated discovery denied")
            restricted = request(origin, "POST", "/_matrix/client/v3/createRoom", {
                "preset": "private_chat", "name": "Restricted Space discussion", "invite": [service],
                "initial_state": [{"type": "m.room.join_rules", "state_key": "", "content": {
                    "join_rule": "restricted", "allow": [{"type": "m.room_membership", "room_id": space}]}}]}, matrix_token)["room_id"]
            request(origin, "PUT", "/_matrix/client/v3/rooms/" + quote(space, safe="") + "/state/m.space.child/" + quote(restricted, safe=""), {"via": [server_name]}, matrix_token)
            request(origin, "POST", project_path + "/rooms/adopt", {"roomId": restricted}, user_token)
            restricted_body = {"projectId": project["id"], "roomId": restricted, "idempotencyKey": "smoke_restricted_bind"}
            bound = request(origin, "POST", "/hagency/api/v1/agents/" + agent["id"] + "/bindings", restricted_body, user_token)
            for _ in range(30):
                if bound["commandState"] == "active":
                    break
                time.sleep(0.5)
                bound = request(origin, "GET", "/hagency/api/v1/commands/agent.bind/smoke_restricted_bind", token=user_token)
            assert bound["commandState"] == "active", "restricted Room provisioning stayed pending"
            assert bound["creation"]["agent"]["id"] == agent["id"] and bound["creation"]["binding"]["roomId"] == restricted
            assert request(origin, "GET", "/_matrix/client/v3/rooms/" + quote(restricted, safe="") + "/state/m.room.join_rules", token=matrix_token)["join_rule"] == "restricted"
            assert request(origin, "GET", "/_matrix/client/v3/rooms/" + quote(restricted, safe="") + "/state/m.room.member/" + quote(agent["puppetMxid"], safe=""), token=matrix_token)["membership"] == "join"
            print("PASS: same permanent Agent bound a restricted Space-child Room through existing Matrix invitation/join APIs without altering join rules")
            visible_rooms = request(origin,"GET",project_path+"/rooms",token=user_token)["rooms"]
            assert {row["roomId"] for row in visible_rooms} == {discussion,restricted}
            roster = request(origin,"GET",room_path+"/agents",token=user_token)["agents"]
            assert len(roster)==1 and roster[0]["agentId"]==agent["id"] and roster[0]["puppetMxid"]==agent["puppetMxid"]
            assert roster[0]["ownerMxid"]==human["user_id"] and "ownerUserId" not in roster[0]
            print("PASS: live registered Room discovery and public Agent roster match actual Matrix membership without exposing internal owner authority")
            entity_id(device["deviceId"], "dev_")
            assert len(device_token) == 64 and len(user_token) == 64
            device_path="/hagency/api/v1/agents/"+agent["id"]+"/execution-device"
            assert agent["executionDeviceId"]==device["deviceId"]
            unchanged=request(origin,"PUT",device_path,{"expectedGeneration":agent["generation"]},device_token)["agent"]
            assert unchanged["generation"]==agent["generation"]
            assert any(d["id"]==device["deviceId"] for d in request(origin,"GET","/hagency/api/v1/devices",token=user_token)["devices"])
            history = request(origin, "POST", "/hagency/api/v1/execution/history", {"agentId": agent["id"], "cursor": None, "snapshot": None}, device_token)["history"]
            assert history["snapshot"]["count"] == 0 and history["executions"] == []
            lease = request(origin, "POST", "/hagency/api/v1/execution/leases/acquire", {"agentId": agent["id"], "ttlMs": 20000, "takeover": False, "historySnapshot": history["snapshot"]}, device_token)["lease"]
            reference = {"agentId": agent["id"], "epoch": lease["epoch"]}
            message = request(origin, "PUT", "/_matrix/client/v3/rooms/" + quote(discussion, safe="") + "/send/m.room.message/human_probe", {
                "msgtype": "m.text", "body": "hello smoke Agent", "m.mentions": {"user_ids": [agent["puppetMxid"]]}}, matrix_token)["event_id"]
            dispatch = None
            for _ in range(15):
                events = request(origin, "POST", "/hagency/api/v1/execution/events/poll", {"lease": reference, "bindingId": binding["id"], "limit": 10}, device_token)["events"]
                dispatch = next((e for e in events if e["eventId"] == message), None)
                if dispatch is not None:
                    break
                time.sleep(0.5)
            assert dispatch is not None, "mentioned event did not reach owner device"
            assert dispatch["bindingId"] == binding["id"] and dispatch["requesterMxid"] == human["user_id"]
            request(origin, "POST", "/hagency/api/v1/execution/events/ack", {"lease": reference, "dispatchId": dispatch["id"]}, device_token)
            start_body = {"lease": reference, "dispatchId": dispatch["id"], "executionId": "smoke_execution"}
            assert request(origin, "POST", "/hagency/api/v1/execution/events/start", start_body, device_token)["execution"]["newlyStarted"]
            assert not request(origin, "POST", "/hagency/api/v1/execution/events/start", start_body, device_token)["execution"]["newlyStarted"]
            processing_path="/hagency/api/v1/execution/events/processing"
            request(origin,"POST",processing_path,start_body,user_token,expected_status=401)
            request(origin,"POST",processing_path,dict(start_body,roomId=discussion),device_token,expected_status=400)
            request(origin,"POST",processing_path,dict(start_body,executionId="wrong-execution"),device_token,expected_status=409)
            processing=request(origin,"POST",processing_path,start_body,device_token)["processing"]
            assert request(origin,"POST",processing_path,start_body,device_token)["processing"]["id"]==processing["id"]
            proof=request(origin,"POST","/hagency/api/v1/execution/events/authorize-tool",start_body,device_token)["dispatch"]
            assert proof["id"]==dispatch["id"] and proof["executionId"]=="smoke_execution" and proof["state"]=="running"
            try:
                request(origin,"POST","/hagency/api/v1/execution/events/authorize-tool",dict(start_body,executionId="different_execution"),device_token)
            except RuntimeError as error:
                assert str(error).startswith("HTTP 409 at /hagency/api/v1/execution/events/authorize-tool:")
            else:raise RuntimeError("tool eligibility proof accepted a foreign execution")
            # Fixture completion checks transport; no AI model is called.
            reply_body = {"lease": reference, "reply": {"dispatchId": dispatch["id"], "executionId": "smoke_execution", "body": "fixture reply, no model invocation"}}
            reply = request(origin, "POST", "/hagency/api/v1/execution/replies", reply_body, device_token)["reply"]
            entity_id(dispatch["id"], "evt_")
            entity_id(binding["id"], "bnd_")
            entity_id(reply["id"], "rep_")
            assert "workerToken" not in reply
            assert request(origin, "POST", "/hagency/api/v1/execution/replies", reply_body, device_token)["reply"]["id"] == reply["id"]
            for _ in range(20):
                sent = sql(DBS[0], "SELECT count(*) FROM hagency_agent_v1.reply_outbox WHERE id='" + reply["id"] + "' AND state='sent' AND matrix_event_id IS NOT NULL", True)
                if int(sent) == 1:
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError("durable reply was not sent by the exact puppet")
            for _ in range(20):
                processing_event=sql(DBS[0],"SELECT matrix_event_id FROM hagency_agent_v1.processing_outbox WHERE id='"+processing["id"]+"' AND state='sent'",True)
                if processing_event:break
                time.sleep(0.5)
            assert processing_event,"durable processing reaction was not sent"
            reaction=request(origin,"GET","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/event/"+quote(processing_event,safe=""),token=matrix_token)
            assert reaction["type"]=="m.reaction" and reaction["sender"]==agent["puppetMxid"]
            assert reaction["content"]=={"m.relates_to":{"rel_type":"m.annotation","event_id":message,"key":"\U0001f440"}}
            assert request(origin,"POST",processing_path,start_body,device_token)["processing"]["matrixEventId"]==processing_event
            assert sql(DBS[0],"SELECT count(*) FROM hagency_agent_v1.processing_outbox WHERE owner_event_id='"+dispatch["id"]+"'",True)=="1"
            print("PASS: exact running device processing receipt sent one stable puppet annotation reaction; same-attempt completed replay returns the same sent receipt")
            print("PASS: real delegated user identity, Space/Room authorization, permanent owner puppet, device lease, mention delivery, distinct ACK/start and idempotent reply outbox")
            request(origin,"POST","/hagency/api/v1/execution/leases/renew",{"lease":reference,"ttlMs":60000},device_token)
            direct_room=request(origin,"POST","/_matrix/client/v3/createRoom",{"preset":"private_chat","is_direct":True,"name":"Owner Agent DM","invite":[agent["puppetMxid"]],"initial_state":[{"type":"m.room.guest_access","state_key":"","content":{"guest_access":"forbidden"}},{"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}}]},matrix_token)["room_id"]
            direct_path="/hagency/api/v1/agents/"+agent["id"]+"/owner-direct"
            direct=request(origin,"POST",direct_path,{"roomId":direct_room},user_token)
            for _ in range(30):
                if direct["commandState"]=="active":break
                time.sleep(0.5)
                direct=request(origin,"POST",direct_path,{"roomId":direct_room},user_token)
            assert direct["commandState"]=="active", "independent owner DM provisioning did not converge"
            direct_binding=direct["creation"]["binding"]
            assert direct_binding["projectId"] is None and direct_binding["scopeKind"]=="owner_direct"
            assert request(origin,"GET",direct_path,token=user_token)["ownerDirectRoomId"]==direct_room
            direct_message=request(origin,"PUT","/_matrix/client/v3/rooms/"+quote(direct_room,safe="")+"/send/m.room.message/owner_direct_probe",{"msgtype":"m.text","body":"direct owner request without mentions"},matrix_token)["event_id"]
            direct_dispatch=None
            for _ in range(30):
                direct_events=request(origin,"POST","/hagency/api/v1/execution/events/poll",{"lease":reference,"bindingId":direct_binding["id"],"limit":10},device_token)["events"]
                direct_dispatch=next((e for e in direct_events if e["eventId"]==direct_message),None)
                if direct_dispatch:break
                time.sleep(0.5)
            assert direct_dispatch is not None,"owner DM request did not route without mentions"
            request(origin,"POST","/hagency/api/v1/execution/events/ack",{"lease":reference,"dispatchId":direct_dispatch["id"]},device_token)
            direct_start={"lease":reference,"dispatchId":direct_dispatch["id"],"executionId":"owner_direct_fixture_execution"}
            request(origin,"POST","/hagency/api/v1/execution/events/start",direct_start,device_token)
            direct_processing=request(origin,"POST",processing_path,direct_start,device_token)["processing"]
            direct_reply=request(origin,"POST","/hagency/api/v1/execution/replies",{"lease":reference,"reply":{"dispatchId":direct_dispatch["id"],"executionId":direct_start["executionId"],"body":"owner DM fixture reply; no model"}},device_token)["reply"]
            for _ in range(30):
                if sql(DBS[0],"SELECT state FROM hagency_agent_v1.reply_outbox WHERE id='"+direct_reply["id"]+"'",True)=="sent":break
                time.sleep(0.5)
            else:raise RuntimeError("owner DM puppet reply did not send")
            for _ in range(20):
                reaction_event=sql(DBS[0],"SELECT matrix_event_id FROM hagency_agent_v1.processing_outbox WHERE id='"+direct_processing["id"]+"' AND state='sent'",True)
                if reaction_event:break
                time.sleep(0.5)
            assert reaction_event,"owner DM reaction was not sent"
            reaction=request(origin,"GET","/_matrix/client/v3/rooms/"+quote(direct_room,safe="")+"/event/"+quote(reaction_event,safe=""),token=matrix_token)
            assert reaction["type"]=="m.reaction" and reaction["sender"]==agent["puppetMxid"]
            assert reaction["content"]=={"m.relates_to":{"rel_type":"m.annotation","event_id":direct_message,"key":"\U0001f440"}}
            print("PASS: private owner DM independent of Project, AS puppet join, unmentioned owner request, assigned-device execution and exact puppet reply")
            if BACKUP_RESTORE:
                # Include actual persisted Matrix media and a configured Pasion
                # password pepper; successful re-login will exercise that pepper.
                media_body=b"isolated Matrix media restoration fixture"
                with urlopen(Request(origin+"/_matrix/media/v3/upload?filename=restore-fixture.txt",data=media_body,headers={"Authorization":"Bearer "+matrix_token,"Content-Type":"text/plain"},method="POST"),timeout=15) as uploaded:
                    media_uri=json.load(uploaded)["content_uri"]
                media_parts=urlparse(media_uri)
                media_download="/_matrix/client/v1/media/download/"+quote(media_parts.netloc,safe="")+"/"+quote(media_parts.path.lstrip("/"),safe="")
                # Keep one authenticated-but-unfinished execution in the snapshot.
                # No model/tool runs; after authority revocation it must be unknown.
                request(origin,"POST","/hagency/api/v1/execution/leases/renew",{"lease":reference,"ttlMs":60000},device_token)
                unfinished_message=request(origin,"PUT","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/send/m.room.message/before_backup_running",{"msgtype":"m.text","body":"unfinished fixture execution","m.mentions":{"user_ids":[agent["puppetMxid"]]}},matrix_token)["event_id"]
                unfinished=None
                for _ in range(30):
                    events=request(origin,"POST","/hagency/api/v1/execution/events/poll",{"lease":reference,"bindingId":binding["id"],"limit":10},device_token)["events"]
                    unfinished=next((e for e in events if e["eventId"]==unfinished_message),None)
                    if unfinished:break
                    time.sleep(0.5)
                assert unfinished is not None
                request(origin,"POST","/hagency/api/v1/execution/events/ack",{"lease":reference,"dispatchId":unfinished["id"]},device_token)
                unfinished_body={"lease":reference,"dispatchId":unfinished["id"],"executionId":"backup_unfinished"}
                request(origin,"POST","/hagency/api/v1/execution/events/start",unfinished_body,device_token)
                # Stop the sole fixture before all three snapshots. No parallel AS
                # instance, no production DB, no credentials in process arguments.
                SERVER.terminate()
                SERVER.wait(timeout=75)
                assert SERVER.returncode == 0, "backup fixture did not stop gracefully"
                SERVER=None
                original_dbs=list(DBS)
                before_commands=sql(DBS[0],"SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY actor_user_id,operation,key),'[]'::jsonb)::text FROM hagency_agent_v1.domain_commands c",True)
                before_as=sql(DBS[0],"SELECT coalesce(jsonb_object_agg(id,digest),'{}'::jsonb)::text FROM hagency_agent_v1.inbound_transactions",True)
                before_reply=sql(DBS[0],"SELECT row_to_json(o)::text FROM hagency_agent_v1.reply_outbox o WHERE id='"+reply["id"]+"'",True)
                before_owner=sql(DBS[0],"SELECT row_to_json(a)::text FROM hagency_agent_v1.agents a WHERE id='"+agent["id"]+"'",True)
                preserved_as=(folder/"data/agent-appservice.json").read_bytes()
                dumps=folder/"database-backup"
                dumps.mkdir(mode=0o700)
                restored=[]
                for index,database in enumerate(original_dbs):
                    dump=dumps/(str(index)+".dump")
                    fd=os.open(str(dump),os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
                    with os.fdopen(fd,"wb") as output:
                        status=subprocess.run(["docker","exec",CONTAINER,"pg_dump","-U",USER,"-d",database,"--format=custom"],stdout=output,stderr=subprocess.PIPE)
                    assert status.returncode==0,"isolated pg_dump failed"
                    target=PREFIX+"_restore_"+str(index)
                    sql("postgres","CREATE DATABASE "+target)
                    CREATED.append(target)
                    with dump.open("rb") as input_file:
                        status=subprocess.run(["docker","exec","-i",CONTAINER,"pg_restore","-U",USER,"-d",target,"--exit-on-error"],stdin=input_file,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                    assert status.returncode==0,"isolated pg_restore failed"
                    restored.append(target)
                # Snapshot complete private data/media and all fixture config/key
                # files. No restore process reads the original live directory.
                source_folder=folder
                snapshot=source_folder/"filesystem-backup"
                snapshot.mkdir(mode=0o700)
                entries=[p for p in source_folder.iterdir() if p.name not in {"server.log","database-backup","filesystem-backup"}]
                def copy_entry(source,target):
                    assert not source.is_symlink(),"fixture backup must not follow symlinks"
                    if source.is_dir():
                        assert not any(p.is_symlink() for p in source.rglob("*")),"fixture backup must not follow nested symlinks"
                        shutil.copytree(source,target)
                    else:shutil.copy2(source,target)
                def manifest(root):
                    return {str(p.relative_to(root)):(hashlib.sha256(p.read_bytes()).digest(),p.stat().st_mode & 0o777) for p in root.rglob("*") if p.is_file()}
                for entry in entries:copy_entry(entry,snapshot/entry.name)
                original_manifest=manifest(snapshot)
                required={"data/agent-appservice.json","data/matrix-signing-key.json","data/pasion-secrets.json","password-pepper","hagency.toml","palpo.toml","pasion.toml"}
                assert required.issubset(original_manifest),"required persistent identity material missing from snapshot"
                for key_file in required:
                    assert original_manifest[key_file][1] & 0o077==0,"persistent private key/config permissions are not private"
                assert any(key.startswith("media/") for key in original_manifest),"actual Matrix uploaded media was not persisted"
                restored_folder=Path(tempfile.mkdtemp(prefix="hagency-agent-restored-"))
                FIXTURE_FOLDERS.append(restored_folder)
                assert restored_folder!=source_folder and restored_folder.stat().st_mode & 0o077==0
                for entry in snapshot.iterdir():copy_entry(entry,restored_folder/entry.name)
                assert manifest(restored_folder)==original_manifest,"filesystem restoration changed contents or modes"
                folder=restored_folder
                # Change only DB URLs and local filesystem roots, retaining origin,
                # issuer, homeserver, namespace and exact private identity bytes.
                for config,index in [("hagency.toml",0),("palpo.toml",1),("pasion.toml",2)]:
                    path=folder/config
                    content=path.read_text()
                    assert dburl(original_dbs[index]) in content
                    content=content.replace(dburl(original_dbs[index]),dburl(restored[index])).replace(str(source_folder),str(restored_folder))
                    assert str(source_folder) not in content,"restored config still points at original fixture files"
                    path.write_text(content)
                    assert path.stat().st_mode & 0o077==0
                for key_file in required-{"hagency.toml","palpo.toml","pasion.toml"}:
                    assert hashlib.sha256((folder/key_file).read_bytes()).digest()==original_manifest[key_file][0]
                # Make every original pathname unavailable, so key/media reads
                # cannot silently fall back to the source tree during restart.
                sealed_source=source_folder.with_name(source_folder.name+"-offline")
                assert not sealed_source.exists()
                source_folder.rename(sealed_source)
                FIXTURE_FOLDERS[FIXTURE_FOLDERS.index(source_folder)]=sealed_source
                assert not source_folder.exists()
                DBS[:]=restored
                assert sql(DBS[0],"SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY actor_user_id,operation,key),'[]'::jsonb)::text FROM hagency_agent_v1.domain_commands c",True)==before_commands
                assert sql(DBS[0],"SELECT coalesce(jsonb_object_agg(id,digest),'{}'::jsonb)::text FROM hagency_agent_v1.inbound_transactions",True)==before_as
                assert sql(DBS[0],"SELECT row_to_json(o)::text FROM hagency_agent_v1.reply_outbox o WHERE id='"+reply["id"]+"'",True)==before_reply
                assert sql(DBS[0],"SELECT row_to_json(a)::text FROM hagency_agent_v1.agents a WHERE id='"+agent["id"]+"'",True)==before_owner
                with (folder/"server.log").open("wb") as restored_log:
                    SERVER=subprocess.Popen([str(binary),"--config",str(folder/"hagency.toml")],cwd=ROOT,stdout=restored_log,stderr=restored_log)
                for _ in range(120):
                    if SERVER.poll() is not None:raise RuntimeError("restored fixture server stopped; private logs retained")
                    try:
                        if request(origin,"GET","/hagency/api/v1/readiness")["startupRoundtripConfirmed"]:break
                    except (URLError,TimeoutError):pass
                    time.sleep(0.5)
                else:raise RuntimeError("restored startup AS gate did not become ready")
                assert (folder/"data/agent-appservice.json").read_bytes()==preserved_as
                for transaction,digest in json.loads(before_as).items():
                    assert json.loads(sql(DBS[0],"SELECT coalesce(jsonb_object_agg(id,digest),'{}'::jsonb)::text FROM hagency_agent_v1.inbound_transactions",True))[transaction]==digest
                matrix_token=native_oauth(origin,(folder/"bootstrap-password").read_text())
                recovered_session=request(origin,"POST","/hagency/api/v1/sessions/pasion",{"accessToken":matrix_token})
                assert recovered_session["userId"]==session["userId"]
                user_token=recovered_session["token"]
                for key_file in required-{"hagency.toml","palpo.toml","pasion.toml"}:
                    assert hashlib.sha256((folder/key_file).read_bytes()).digest()==original_manifest[key_file][0],"restart rotated restored identity/pepper"
                with urlopen(Request(origin+media_download,headers={"Authorization":"Bearer "+matrix_token}),timeout=15) as downloaded:
                    assert downloaded.read()==media_body,"restored real Matrix media changed"
                recovered_agent=request(origin,"GET","/hagency/api/v1/agents/"+agent["id"],token=user_token)["agent"]
                assert recovered_agent["ownerUserId"]==agent["ownerUserId"] and recovered_agent["puppetMxid"]==agent["puppetMxid"]
                recovered_command=request(origin,"GET","/hagency/api/v1/commands/agent.create/smoke_create",token=user_token)
                assert recovered_command["creation"]["agent"]["id"]==agent["id"]
                old_device_token=device_token
                request(origin,"DELETE","/hagency/api/v1/devices/"+device["deviceId"],token=user_token)
                request(origin,"POST","/hagency/api/v1/execution/events/poll",{"lease":reference,"bindingId":binding["id"],"limit":10},old_device_token,expected_status=401)
                device=request(origin,"POST","/hagency/api/v1/devices",{"installationId":"restored_smoke_device","name":"fresh recovery authority"},user_token)
                device_token=device["token"]
                request(origin,"POST","/hagency/api/v1/execution/leases/acquire",{"agentId":agent["id"],"ttlMs":60000,"takeover":True,"historySnapshot":history["snapshot"]},device_token,expected_status=401)
                agent=request(origin,"PUT",device_path,{"expectedGeneration":recovered_agent["generation"]},device_token)["agent"]
                restored_history=request(origin,"POST","/hagency/api/v1/execution/history",{"agentId":agent["id"],"cursor":None,"snapshot":None},device_token)["history"]
                assert restored_history["snapshot"]["count"] >= 2 and restored_history["nextCursor"] is None
                assert all("body" not in entry for entry in restored_history["executions"])
                recovered_lease=request(origin,"POST","/hagency/api/v1/execution/leases/acquire",{"agentId":agent["id"],"ttlMs":60000,"takeover":True,"historySnapshot":restored_history["snapshot"]},device_token)["lease"]
                assert recovered_lease["epoch"]>reference["epoch"]
                reference={"agentId":agent["id"],"epoch":recovered_lease["epoch"]}
                for _ in range(30):
                    state=sql(DBS[0],"SELECT state FROM hagency_agent_v1.owner_events WHERE id='"+unfinished["id"]+"'",True)
                    if state=="unknown":break
                    time.sleep(0.5)
                assert state=="unknown","unfinished restored execution was not fenced unknown"
                request(origin,"POST","/hagency/api/v1/execution/events/start",dict(unfinished_body,lease=reference),device_token,expected_status=401)
                recovered_reply=request(origin,"POST","/hagency/api/v1/execution/replies/reconcile-known",dict(reply_body,lease=reference),device_token)["reply"]
                saved=json.loads(before_reply)
                assert recovered_reply["state"]=="sent" and recovered_reply["matrixEventId"]==saved["matrix_event_id"] and recovered_reply["matrixTxnId"]==saved["matrix_txn_id"] and recovered_reply["body"]==saved["body"]
                original_event=request(origin,"GET","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/event/"+quote(saved["matrix_event_id"],safe=""),token=matrix_token)
                assert original_event["sender"]==agent["puppetMxid"] and original_event["content"]["body"]==saved["body"]
                fresh_message=request(origin,"PUT","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/send/m.room.message/after_restore_probe",{"msgtype":"m.text","body":"post restore fixture","m.mentions":{"user_ids":[agent["puppetMxid"]]}},matrix_token)["event_id"]
                fresh=None
                for _ in range(30):
                    events=request(origin,"POST","/hagency/api/v1/execution/events/poll",{"lease":reference,"bindingId":binding["id"],"limit":10},device_token)["events"]
                    assert all(e["id"]!=unfinished["id"] and e["id"]!=dispatch["id"] for e in events)
                    fresh=next((e for e in events if e["eventId"]==fresh_message),None)
                    if fresh:break
                    time.sleep(0.5)
                assert fresh is not None
                request(origin,"POST","/hagency/api/v1/execution/events/ack",{"lease":reference,"dispatchId":fresh["id"]},device_token)
                request(origin,"POST","/hagency/api/v1/execution/events/start",{"lease":reference,"dispatchId":fresh["id"],"executionId":"after_restore_execution"},device_token)
                fresh_reply=request(origin,"POST","/hagency/api/v1/execution/replies",{"lease":reference,"reply":{"dispatchId":fresh["id"],"executionId":"after_restore_execution","body":"post restore reply without model"}},device_token)["reply"]
                for _ in range(30):
                    if sql(DBS[0],"SELECT state FROM hagency_agent_v1.reply_outbox WHERE id='"+fresh_reply["id"]+"'",True)=="sent":break
                    time.sleep(0.5)
                else:raise RuntimeError("fresh restored request did not complete Matrix reply")
                assert sql(DBS[0],"SELECT state FROM hagency_agent_v1.owner_events WHERE id='"+unfinished["id"]+"'",True)=="unknown"
                assert sql(DBS[0],"SELECT row_to_json(o)::text FROM hagency_agent_v1.reply_outbox o WHERE id='"+reply["id"]+"'",True)==before_reply
                print("PASS: stopped three-database and independent private filesystem restore; AS/Matrix/Pasion keys and pepper byte-identical, real media retained, permanent identity/digests/stable sent reply retained, old device revoked, unfinished execution unknown, fresh native-login/device request roundtrip completed without inference")
            # Actual membership loss is committed by the failed sensitive API,
            # even if Matrix rejoin happens before the periodic controller sweep.
            request(origin,"POST","/hagency/api/v1/sessions/current/renew",{"accessToken":matrix_token},user_token)
            request(origin,"POST","/hagency/api/v1/execution/leases/renew",{"lease":reference,"ttlMs":60000},device_token)
            membership_message=request(origin,"PUT","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/send/m.room.message/membership_fence_probe",{"msgtype":"m.text","body":"membership fixture execution","m.mentions":{"user_ids":[agent["puppetMxid"]]}},matrix_token)["event_id"]
            membership_dispatch=None
            for _ in range(30):
                events=request(origin,"POST","/hagency/api/v1/execution/events/poll",{"lease":reference,"bindingId":binding["id"],"limit":10},device_token)["events"]
                membership_dispatch=next((e for e in events if e["eventId"]==membership_message),None)
                if membership_dispatch:break
                time.sleep(0.5)
            assert membership_dispatch is not None
            request(origin,"POST","/hagency/api/v1/execution/events/ack",{"lease":reference,"dispatchId":membership_dispatch["id"]},device_token)
            membership_start={"lease":reference,"dispatchId":membership_dispatch["id"],"executionId":"membership_fence_execution"}
            request(origin,"POST","/hagency/api/v1/execution/events/start",membership_start,device_token)
            binding_path="/hagency/api/v1/bindings/"+binding["id"]
            before_kick=request(origin,"GET",binding_path,token=user_token)["binding"]
            request(origin,"POST","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/kick",{"user_id":agent["puppetMxid"],"reason":"isolated membership fencing fixture"},matrix_token)
            request(origin,"POST","/hagency/api/v1/execution/events/authorize-tool",membership_start,device_token,expected_status=401)
            paused=request(origin,"GET",binding_path,token=user_token)["binding"]
            assert paused["state"] in ("leaving", "left") and paused["generation"]>before_kick["generation"]
            # Fixture-only AS invitation/join restores Matrix membership, not
            # Hagency authorization or an old dispatch generation.
            request(origin,"POST","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/invite"+query,{"user_id":agent["puppetMxid"]},registration["as_token"])
            request(origin,"POST","/_matrix/client/v3/join/"+quote(discussion,safe="")+"?user_id="+quote(agent["puppetMxid"],safe=""),{},registration["as_token"])
            assert request(origin,"GET",binding_path,token=user_token)["binding"]["state"] in ("leaving", "left")
            request(origin,"POST","/hagency/api/v1/execution/events/authorize-tool",membership_start,device_token,expected_status=401)
            request(origin,"POST",binding_path+"/resume",token=user_token,expected_status=409)
            for _ in range(30):
                departed = request(origin,"GET",binding_path,token=user_token)["binding"]
                member = request(origin,"GET","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/state/m.room.member/"+quote(agent["puppetMxid"],safe=""),token=matrix_token)
                if departed["state"] == "left" and member["membership"] == "leave": break
                time.sleep(0.5)
            else: raise RuntimeError("retired membership did not converge to physical departure")
            rebound_body = {"projectId": project["id"], "roomId": discussion, "idempotencyKey": "membership_rebind"}
            rebound = request(origin,"POST","/hagency/api/v1/agents/"+agent["id"]+"/bindings",rebound_body,user_token)
            for _ in range(30):
                if rebound["commandState"] == "active": break
                time.sleep(0.5)
                rebound = request(origin,"GET","/hagency/api/v1/commands/agent.bind/membership_rebind",token=user_token)
            assert rebound["commandState"] == "active"
            resumed = rebound["creation"]["binding"]
            assert resumed["id"] == binding["id"] and resumed["generation"]>paused["generation"]
            request(origin,"POST","/hagency/api/v1/execution/events/start",membership_start,device_token,expected_status=401)
            request(origin,"POST","/hagency/api/v1/execution/events/authorize-tool",membership_start,device_token,expected_status=401)
            # The restricted second Room remains authorized throughout this loss.
            second_binding=bound["creation"]["binding"]
            assert request(origin,"GET","/hagency/api/v1/bindings/"+second_binding["id"],token=user_token)["binding"]["state"]=="active"
            after_resume_message=request(origin,"PUT","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/send/m.room.message/after_membership_resume",{"msgtype":"m.text","body":"fresh request after explicit rebind","m.mentions":{"user_ids":[agent["puppetMxid"]]}},matrix_token)["event_id"]
            resumed_dispatch=None
            for _ in range(30):
                events=request(origin,"POST","/hagency/api/v1/execution/events/poll",{"lease":reference,"bindingId":binding["id"],"limit":10},device_token)["events"]
                assert all(e["id"]!=membership_dispatch["id"] for e in events)
                resumed_dispatch=next((e for e in events if e["eventId"]==after_resume_message),None)
                if resumed_dispatch:break
                time.sleep(0.5)
            assert resumed_dispatch is not None and resumed_dispatch["bindingGeneration"]==resumed["generation"]
            request(origin,"POST","/hagency/api/v1/execution/events/ack",{"lease":reference,"dispatchId":resumed_dispatch["id"]},device_token)
            request(origin,"POST","/hagency/api/v1/execution/events/start",{"lease":reference,"dispatchId":resumed_dispatch["id"],"executionId":"membership_resumed_execution"},device_token)
            resumed_reply=request(origin,"POST","/hagency/api/v1/execution/replies",{"lease":reference,"reply":{"dispatchId":resumed_dispatch["id"],"executionId":"membership_resumed_execution","body":"explicit rebind fixture reply without model"}},device_token)["reply"]
            for _ in range(30):
                if sql(DBS[0],"SELECT state FROM hagency_agent_v1.reply_outbox WHERE id='"+resumed_reply["id"]+"'",True)=="sent":break
                time.sleep(0.5)
            else:raise RuntimeError("fresh resumed-generation reply did not reach Matrix")
            assert sql(DBS[0],"SELECT state FROM hagency_agent_v1.owner_events WHERE id='"+membership_dispatch["id"]+"'",True)=="unknown"
            print("PASS: actual puppet kick persisted departure through denied tool authorization; rejoin and resume did not restore authority, explicit owner rebind fenced old dispatch, independent binding remained active and fresh request replied")
            request(origin,"POST","/hagency/api/v1/sessions/current/renew",{"accessToken":matrix_token},user_token)
            request(origin,"POST","/hagency/api/v1/execution/leases/renew",{"lease":reference,"ttlMs":60000},device_token)
            power_message=request(origin,"PUT","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/send/m.room.message/power_fence_probe",{"msgtype":"m.text","body":"power fixture execution","m.mentions":{"user_ids":[agent["puppetMxid"]]}},matrix_token)["event_id"]
            power_dispatch=None
            for _ in range(30):
                events=request(origin,"POST","/hagency/api/v1/execution/events/poll",{"lease":reference,"bindingId":binding["id"],"limit":10},device_token)["events"]
                power_dispatch=next((e for e in events if e["eventId"]==power_message),None)
                if power_dispatch:break
                time.sleep(0.5)
            assert power_dispatch is not None
            request(origin,"POST","/hagency/api/v1/execution/events/ack",{"lease":reference,"dispatchId":power_dispatch["id"]},device_token)
            power_start={"lease":reference,"dispatchId":power_dispatch["id"],"executionId":"power_fence_execution"}
            request(origin,"POST","/hagency/api/v1/execution/events/start",power_start,device_token)
            power_path="/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/state/m.room.power_levels"
            original_power=request(origin,"GET",power_path,token=matrix_token)
            denied_power=json.loads(json.dumps(original_power))
            denied_power.setdefault("events",{})["m.room.message"]=100
            denied_power.setdefault("users",{})[agent["puppetMxid"]]=0
            request(origin,"PUT",power_path,denied_power,matrix_token)
            request(origin,"POST","/hagency/api/v1/execution/events/authorize-tool",power_start,device_token,expected_status=401)
            assert sql(DBS[0],"SELECT state FROM hagency_agent_v1.owner_events WHERE id='"+power_dispatch["id"]+"'",True)=="unknown"
            request(origin,"PUT",power_path,original_power,matrix_token)
            request(origin,"POST","/hagency/api/v1/execution/events/authorize-tool",power_start,device_token,expected_status=409)
            assert sql(DBS[0],"SELECT count(*) FROM hagency_agent_v1.reply_outbox WHERE owner_event_id='"+power_dispatch["id"]+"'",True)=="0"
            print("PASS: real Matrix power-level speaking denial permanently fenced running dispatch; restoring power did not authorize old tool execution or create a reply")
            request(origin,"POST","/hagency/api/v1/sessions/current/renew",{"accessToken":matrix_token},user_token)
            request(origin,"DELETE","/hagency/api/v1/agents/"+agent["id"],token=user_token)
            for _ in range(30):
                retired=request(origin,"GET","/hagency/api/v1/agents/"+agent["id"],token=user_token)["agent"]
                if retired["state"]=="retired":break
                time.sleep(0.5)
            else:raise RuntimeError("durable retirement did not converge")
            # Simulate an external Matrix join that completed after retirement.
            # These are fixture-only AS operations, never exposed to a user.
            request(origin,"POST","/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/invite"+query,{"user_id":agent["puppetMxid"]},registration["as_token"])
            request(origin,"POST","/_matrix/client/v3/join/"+quote(discussion,safe="")+"?user_id="+quote(agent["puppetMxid"],safe=""),{},registration["as_token"])
            member_path="/_matrix/client/v3/rooms/"+quote(discussion,safe="")+"/state/m.room.member/"+quote(agent["puppetMxid"],safe="")+query
            for _ in range(30):
                if request(origin,"GET",member_path,token=registration["as_token"])["membership"]=="leave":break
                time.sleep(0.5)
            else:raise RuntimeError("late Matrix join escaped terminal reconciliation")
            assert request(origin,"GET","/hagency/api/v1/agents/"+agent["id"],token=user_token)["agent"]["ownerUserId"]==session["userId"]
            print("PASS: retirement physically left Room; a late real Matrix join was removed while permanent owner identity remained intact")
            SUCCESS = True
finally:
    shutdown_failed = False
    if SERVER is not None and SERVER.poll() is None:
        SERVER.terminate()
        try:
            SERVER.wait(timeout=75)
            shutdown_failed = SERVER.returncode != 0
        except subprocess.TimeoutExpired:
            shutdown_failed = True
            SERVER.kill()
            SERVER.wait()
    cleanup_failures = 0
    for database in reversed(CREATED):
        try:
            sql("postgres", "DROP DATABASE " + database + " WITH (FORCE)")
        except RuntimeError:
            # Attempt every original/restored database even if one drop fails.
            cleanup_failures += 1
    for fixture_folder in reversed(FIXTURE_FOLDERS):
        if SUCCESS and not cleanup_failures and not shutdown_failed:
            shutil.rmtree(fixture_folder)
        else:
            print("Private failure fixture retained at " + str(fixture_folder))
    if cleanup_failures:
        raise RuntimeError("isolated fixture database cleanup failed for " + str(cleanup_failures) + " databases")

    if SUCCESS and shutdown_failed:
        raise RuntimeError("isolated server did not finish its bounded graceful shutdown")
