#!/usr/bin/env python3
"""Hosted Agent builds an app, end to end over the client API (needs the
concierge running the sandboxed-build binary with its preview server):
a fresh user on beta DMs the concierge "create agent", then asks the Agent in
its room to build a hello-world page. PASS only if the Agent's reply contains
the Agent's preview URL AND fetching that URL returns 200 with
"Hello from Hagency" in the body. Empty replies, the server's placeholder
notice, or a reply without the link are failures. Prints timings.

usage: demo-build-app-e2e.py --user NEWNAME [--server http://127.0.0.1:8092]
       [--server-name localhost:8452] [--concierge hagency] [--preview-port 8130]
"""
import argparse, importlib.util, os, re, sys, time, urllib.parse, urllib.request

spec = importlib.util.spec_from_file_location("beat7", os.path.join(os.path.dirname(__file__), "demo-concierge-e2e.py"))
beat7 = importlib.util.module_from_spec(spec); spec.loader.exec_module(beat7)
call, log, wait_for, notices_from = beat7.call, beat7.log, beat7.wait_for, beat7.notices_from

EXPECTED = "Hello from Hagency"
REQUEST = "Build a hello-world web page that says Hello from Hagency"
placeholder = lambda t: "request received" in t.lower() or "请求已收到" in t

def send(origin, token, room, text, mention=None):
    body = {"msgtype": "m.text", "body": text}
    if mention:
        body["m.mentions"] = {"user_ids": [mention]}
    call("PUT", f"{origin}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/send/m.room.message/t{time.time_ns()}", body, token)

def joined(sync, room, user):
    r = sync.get("rooms", {}).get("join", {}).get(room, {})
    return any(ev.get("state_key") == user and ev["content"].get("membership") == "join"
               for part in ("state", "timeline") for ev in r.get(part, {}).get("events", []))

def fetch(url):
    # The preview is on 127.0.0.1: never through a proxy.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(url, timeout=10) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, ""
    except OSError as e:
        return 0, str(e)

def create_hosted_agent(o, token, concierge):
    """DM the concierge; returns (name, puppet, room) from its "Created" notice."""
    status, body = call("POST", f"{o}/_matrix/client/v3/createRoom", {"is_direct": True, "preset": "trusted_private_chat", "invite": [concierge]}, token)
    dm = body.get("room_id") or sys.exit(f"createRoom: {status} {body}")
    ok, since = wait_for(o, token, lambda s: joined(s, dm, concierge), 60)
    if not ok:
        sys.exit("FAIL: the concierge did not join the DM")
    send(o, token, dm, "create agent")
    created, _ = wait_for(o, token, lambda s: [n for n in notices_from(s, dm, concierge) if "Created" in n or "could not" in n or "already have" in n], 240, since)
    if not created or "Created" not in created[-1]:
        sys.exit(f"FAIL: no Agent created: {created[-1] if created else 'no reply'}")
    m = re.match(r"Created (\S+) \((@[^)]+)\)\. I invited you to its room (\S+?);", created[-1])
    if not m:
        sys.exit(f"FAIL: unexpected creation notice: {created[-1]}")
    log(f"concierge: {created[-1]}")
    if "can chat but not build" in created[-1]:
        sys.exit("FAIL: the concierge created a chat-only Agent (sandboxed build rule not saved)")
    return m.group(1), m.group(2), m.group(3)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8092")
    ap.add_argument("--server-name", default="localhost:8452")
    ap.add_argument("--concierge", default="hagency")
    ap.add_argument("--preview-port", type=int, default=8130)
    ap.add_argument("--user", required=True, help="a NEW username (one hosted Agent per user)")
    ap.add_argument("--timeout", type=int, default=600, help="seconds to wait for the built-app reply")
    a = ap.parse_args()
    o, password, concierge = a.server, f"Demo-{a.user}-2026-pass", f"@{a.concierge}:{a.server_name}"
    beat7.register(o, a.user, password)
    token, me = beat7.matrix_login(o, a.user, password)
    log(f"signed in as {me}")
    t0 = time.time()
    name, puppet, room = create_hosted_agent(o, token, concierge)
    t_created = time.time()
    link = f"http://127.0.0.1:{a.preview_port}/{urllib.parse.quote(name, safe='-_.')}/"
    call("POST", f"{o}/_matrix/client/v3/join/{urllib.parse.quote(room, safe='')}", {}, token)
    ok, since = wait_for(o, token, lambda s: joined(s, room, puppet), 120)
    if not ok:
        sys.exit("FAIL: the Agent puppet did not join its room")
    t_joined = time.time()
    send(o, token, room, f"{puppet}: {REQUEST}", puppet)
    t_asked = time.time()
    log(f"asked {name}: {REQUEST}")
    replies = []
    def linked(sync):
        new = [n for n in notices_from(sync, room, puppet) if n.strip() and not placeholder(n)]
        replies.extend(new)
        return [n for n in new if link in n]
    found, _ = wait_for(o, token, linked, a.timeout, since)
    t_reply = time.time()
    if not found:
        last = replies[-1][:300] if replies else "nothing but the placeholder"
        sys.exit(f"FAIL: no reply with {link} within {a.timeout}s; last reply: {last}")
    log(f"agent: {found[-1][:300]}")
    status, body = fetch(link)
    t_fetched = time.time()
    log(f"GET {link} -> {status}, {len(body)} bytes")
    if status != 200 or EXPECTED not in body:
        sys.exit(f"FAIL: the preview did not serve the page (status {status}): {body[:200]!r}")
    log(f"timings: create {t_created-t0:.0f}s, puppet joined +{t_joined-t_created:.0f}s, "
        f"build reply {t_reply-t_asked:.0f}s, fetch {t_fetched-t_reply:.1f}s, total {t_fetched-t0:.0f}s")
    print("PASS")

if __name__ == "__main__":
    main()
