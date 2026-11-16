#!/usr/bin/env python3
"""Beat 7 end to end over the Matrix client API: a fresh user registers on beta,
DMs the concierge "create agent", gets a hosted Agent and a room, asks the
Agent something there, then asks the concierge "status". Exits non-zero when any
step is missing; empty replies count as failure.

usage: demo-concierge-e2e.py [--server http://127.0.0.1:8092] [--server-name localhost:8452]
                             [--concierge hagency] [--user dan] [--password-file FILE]
"""
import argparse, json, sys, time, urllib.parse, urllib.request

def call(method, url, body=None, token=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={"content-type": "application/json"})
    if token:
        req.add_header("authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")

def log(msg):
    print(time.strftime("%H:%M:%S"), msg, flush=True)

def register(origin, user, password):
    status, body = call("POST", f"{origin}/auth/api/v1/auth/login", {"username": user, "password": password})
    if body.get("status") == "success":
        log(f"{user} already registered"); return
    status, body = call("POST", f"{origin}/auth/api/v1/auth/register", {"username": user, "password": password, "password_confirm": password})
    if body.get("status") != "success":
        sys.exit(f"register {user}: {status} {body}")
    rid, step = body["id"], body.get("next_step")
    for _ in range(4):
        path, payload = ("display-name", {"display_name": user, "skip": False}) if step == "display_name" else ("finish", {})
        status, body = call("POST", f"{origin}/auth/api/v1/auth/register/{rid}/{path}", payload)
        if body.get("status") != "success":
            sys.exit(f"register step {path}: {status} {body}")
        if path == "finish":
            log(f"registered {user}"); return
        step = body.get("next_step")
    sys.exit("registration did not finish")

def matrix_login(origin, user, password):
    """Pasion provisions the Matrix user a few seconds after registration."""
    for _ in range(30):
        status, body = call("POST", f"{origin}/_matrix/client/v3/login", {"type": "m.login.password", "identifier": {"type": "m.id.user", "user": user}, "password": password, "initial_device_display_name": "beat7 e2e"})
        if "access_token" in body:
            return body["access_token"], body["user_id"]
        if body.get("errcode") == "M_LIMIT_EXCEEDED":
            wait = body.get("retry_after_ms", 10000) / 1000
            log(f"login rate-limited; waiting {wait:.0f}s"); time.sleep(wait + 1); continue
        if "not provisioned" not in body.get("error", ""):
            break
        time.sleep(2)
    sys.exit(f"matrix login: {status} {body}")

def wait_for(origin, token, predicate, timeout, since=None):
    """Long-poll /sync until predicate(sync) returns a value."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        url = f"{origin}/_matrix/client/v3/sync?timeout=20000" + (f"&since={since}" if since else "")
        status, sync = call("GET", url, token=token)
        since = sync.get("next_batch", since)
        found = predicate(sync)
        if found:
            return found, since
    return None, since

def notices_from(sync, room, sender):
    out = []
    for ev in sync.get("rooms", {}).get("join", {}).get(room, {}).get("timeline", {}).get("events", []):
        if ev.get("type") == "m.room.message" and ev.get("sender") == sender and ev["content"].get("body"):
            out.append(ev["content"]["body"])
    return out

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8092")
    ap.add_argument("--server-name", default="localhost:8452")
    ap.add_argument("--concierge", default="hagency")
    ap.add_argument("--user", default="dan", help="use a NEW username per run: the second request from one user is refused by design")
    ap.add_argument("--password-file", default=None)
    a = ap.parse_args()
    password = open(a.password_file).read().strip() if a.password_file else f"Demo-{a.user}-2026-pass"
    concierge = f"@{a.concierge}:{a.server_name}"
    register(a.server, a.user, password)
    token, me = matrix_login(a.server, a.user, password)
    log(f"signed in as {me}")
    status, body = call("POST", f"{a.server}/_matrix/client/v3/createRoom", {"is_direct": True, "preset": "trusted_private_chat", "invite": [concierge]}, token)
    dm = body.get("room_id") or sys.exit(f"createRoom: {status} {body}")
    log(f"DM with {concierge}: {dm}")
    _, since = wait_for(a.server, token, lambda s: concierge in json.dumps(s.get("rooms", {}).get("join", {}).get(dm, {}).get("state", {})) or any(
        ev.get("state_key") == concierge and ev["content"].get("membership") == "join" for ev in s.get("rooms", {}).get("join", {}).get(dm, {}).get("timeline", {}).get("events", [])), 60)
    call("PUT", f"{a.server}/_matrix/client/v3/rooms/{urllib.parse.quote(dm, safe='')}/send/m.room.message/e2e{int(time.time())}", {"msgtype": "m.text", "body": "create agent"}, token)
    log("sent: create agent")
    reply, since = wait_for(a.server, token, lambda s: notices_from(s, dm, concierge), 240, since)
    if not reply:
        sys.exit("FAIL: no reply from the concierge within 240 s")
    log(f"concierge: {reply[-1]}")
    if "already have" in reply[-1]:
        # Second run as the same user: the cap holds; find the room via status.
        call("PUT", f"{a.server}/_matrix/client/v3/rooms/{urllib.parse.quote(dm, safe='')}/send/m.room.message/s0{int(time.time())}", {"msgtype": "m.text", "body": "status"}, token)
        stat, since = wait_for(a.server, token, lambda s: [n for n in notices_from(s, dm, concierge) if " in !" in n], 60, since)
        if not stat:
            sys.exit("FAIL: no status reply for the existing Agent")
        room = stat[-1].split(" in ")[1].split(" ")[0]
        log(f"existing agent room {room}")
    elif "Created" not in reply[-1]:
        sys.exit("FAIL: the concierge did not create an Agent")
    else:
        invites, since = wait_for(a.server, token, lambda s: list(s.get("rooms", {}).get("invite", {}).keys()), 60, None)
        if not invites:
            sys.exit("FAIL: no room invite arrived")
        room = invites[0]
        call("POST", f"{a.server}/_matrix/client/v3/join/{urllib.parse.quote(room, safe='')}", {}, token)
        log(f"joined agent room {room}")
    puppet = None
    def find_puppet(s):
        for ev in s.get("rooms", {}).get("join", {}).get(room, {}).get("state", {}).get("events", []) + s.get("rooms", {}).get("join", {}).get(room, {}).get("timeline", {}).get("events", []):
            if ev.get("type") == "m.room.member" and ev.get("state_key", "").startswith("@_hagency_agt_") and ev["content"].get("membership") == "join":
                return ev["state_key"]
    puppet, since = wait_for(a.server, token, find_puppet, 120, None)
    if not puppet:
        sys.exit("FAIL: the Agent puppet did not join the room")
    log(f"agent puppet joined: {puppet}")
    # Anyone but the owner must mention the Agent (server rule); Element does
    # this with the @-pill, the API with m.mentions.
    call("PUT", f"{a.server}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/send/m.room.message/q{int(time.time())}", {"msgtype": "m.text", "body": f"{puppet}: Reply with exactly one word: pong", "m.mentions": {"user_ids": [puppet]}}, token)
    # The server posts a "request received, Agent not processing yet" notice
    # when the runtime is idle; that is not an answer (R14: no vacuous pass).
    placeholder = lambda text: "请求已收到" in text or "request received" in text.lower()
    answer, since = wait_for(a.server, token, lambda s: [n for n in notices_from(s, room, puppet) if not placeholder(n)], 300, since)
    if not answer:
        sys.exit("FAIL: the Agent did not answer in its room (only the server's waiting notice, or nothing)")
    log(f"agent: {answer[-1][:120]}")
    call("PUT", f"{a.server}/_matrix/client/v3/rooms/{urllib.parse.quote(dm, safe='')}/send/m.room.message/s{int(time.time())}", {"msgtype": "m.text", "body": "status"}, token)
    stat, _ = wait_for(a.server, token, lambda s: [n for n in notices_from(s, dm, concierge) if "credit" in n], 60, None)
    if not stat:
        sys.exit("FAIL: no status reply")
    log(f"status: {stat[-1]}")
    # Owner controls over chat (item C): pause → the Agent stays silent, resume → it answers, history lists it.
    name = stat[-1].split(":")[0].strip()
    def say(text, tag):
        call("PUT", f"{a.server}/_matrix/client/v3/rooms/{urllib.parse.quote(dm, safe='')}/send/m.room.message/{tag}{int(time.time())}", {"msgtype": "m.text", "body": text}, token)
    say(f"pause {name}", "p")
    paused, since = wait_for(a.server, token, lambda s: [n for n in notices_from(s, dm, concierge) if "paused" in n], 60, None)
    if not paused:
        sys.exit("FAIL: no pause confirmation")
    log(f"pause: {paused[-1].splitlines()[-1]}")
    # Paused means silent: a mention in the Agent's room must get no answer.
    call("PUT", f"{a.server}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/send/m.room.message/pq{int(time.time())}", {"msgtype": "m.text", "body": f"{puppet}: Reply with exactly one word: ping", "m.mentions": {"user_ids": [puppet]}}, token)
    answered, since2 = wait_for(a.server, token, lambda s: [n for n in notices_from(s, room, puppet) if "ping" in n.lower() and not placeholder(n)], 45, None)
    if answered:
        sys.exit(f"FAIL: the Agent answered while paused: {answered[-1][:80]}")
    log("paused agent stayed silent for 45 s (good)")
    say(f"history {name}", "h")
    hist, since = wait_for(a.server, token, lambda s: [n for n in notices_from(s, dm, concierge) if "owner commands:" in n], 60, since)
    if not hist:
        sys.exit("FAIL: no history reply")
    log("history: " + " | ".join(hist[-1].splitlines()[:3]))
    say(f"resume {name}", "r")
    resumed, since = wait_for(a.server, token, lambda s: [n for n in notices_from(s, dm, concierge) if "shared again" in n or "resumed" in n], 90, since)
    if not resumed:
        sys.exit("FAIL: no resume confirmation")
    log(f"resume: {resumed[-1].splitlines()[-1]}")
    say(f"stop sharing {name}", "s")
    stopped, since = wait_for(a.server, token, lambda s: [n for n in notices_from(s, dm, concierge) if "no longer shared" in n], 90, since)
    if not stopped:
        sys.exit("FAIL: no stop-sharing confirmation")
    log(f"stop sharing: {stopped[-1].splitlines()[-2] if len(stopped[-1].splitlines())>1 else stopped[-1]}")
    say(f"resume {name}", "r2")
    wait_for(a.server, token, lambda s: [n for n in notices_from(s, dm, concierge) if "shared again" in n], 90, since)
    print("PASS")

if __name__ == "__main__":
    main()
