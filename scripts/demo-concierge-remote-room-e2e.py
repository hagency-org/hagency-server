#!/usr/bin/env python3
"""Concierge brings a hosted Agent into a plain room hosted on ANOTHER server
(scene 6 with the project room on alpha). A fresh beta user gets a hosted Agent;
a fresh alpha user creates a plain room and invites them; the beta user first
invites their Agent directly (what Element users do), then invites the concierge.
The concierge must join over federation, bring the Agent (asking for moderator
if the server needs it; the alpha room creator grants it), and the Agent must
answer there. Exits non-zero on any missing step; empty replies are failures.

usage: demo-concierge-remote-room-e2e.py --user NEWBETA --host-user NEWALPHA
       [--server http://127.0.0.1:8092] [--server-name localhost:8452]
       [--alpha http://127.0.0.1:8090] [--alpha-name localhost:8450] [--concierge hagency]
"""
import argparse, importlib.util, os, sys, time, urllib.parse

here = os.path.dirname(__file__)
def load(name, file):
    spec = importlib.util.spec_from_file_location(name, os.path.join(here, file))
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module); return module
beat7 = load("beat7", "demo-concierge-e2e.py")
rooms = load("rooms", "demo-concierge-rooms-e2e.py")
call, log, wait_for, notices_from = beat7.call, beat7.log, beat7.wait_for, beat7.notices_from

def member_is(sync, room, user, membership):
    return any(ev.get("state_key") == user and ev["content"].get("membership") == membership
               for part in ("state", "timeline") for ev in sync.get("rooms", {}).get("join", {}).get(room, {}).get(part, {}).get("events", []))

def invite(origin, token, room, user):
    status, body = call("POST", f"{origin}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/invite", {"user_id": user}, token)
    if status != 200:
        sys.exit(f"invite {user}: {status} {body}")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8092")
    ap.add_argument("--server-name", default="localhost:8452")
    ap.add_argument("--alpha", default="http://127.0.0.1:8090")
    ap.add_argument("--alpha-name", default="localhost:8450")
    ap.add_argument("--concierge", default="hagency")
    ap.add_argument("--user", required=True, help="a NEW beta username (one hosted Agent per user)")
    ap.add_argument("--host-user", required=True, help="a NEW alpha username that owns the room")
    a = ap.parse_args()
    b, al, concierge = a.server, a.alpha, f"@{a.concierge}:{a.server_name}"
    t0 = time.time()
    beat7.register(b, a.user, f"Demo-{a.user}-2026-pass")
    token, me = beat7.matrix_login(b, a.user, f"Demo-{a.user}-2026-pass")
    beat7.register(al, a.host_user, f"Demo-{a.host_user}-2026-pass")
    host_token, host = beat7.matrix_login(al, a.host_user, f"Demo-{a.host_user}-2026-pass")

    dm = rooms.create_room(b, token, "concierge", [concierge], False)
    wait_for(b, token, lambda s: member_is(s, dm, concierge, "join"), 60)
    rooms.send(b, token, dm, "create agent")
    created, since = wait_for(b, token, lambda s: [n for n in notices_from(s, dm, concierge) if "Created" in n], 240)
    if not created:
        sys.exit("FAIL: no Agent created")
    puppet = created[-1].split("(")[1].split(")")[0]
    log(f"created {puppet} in {time.time()-t0:.0f}s")

    plain = rooms.create_room(al, host_token, "e2e remote plain", [me], False)
    log(f"{host} created {plain} on alpha and invited {me}")
    for _ in range(30):
        status, body = call("POST", f"{b}/_matrix/client/v3/join/{urllib.parse.quote(plain, safe='')}?server_name={a.alpha_name}", {}, token)
        if status == 200: break
        time.sleep(1)
    else:
        sys.exit(f"FAIL: {me} could not join {plain}: {status} {body}")
    t1 = time.time()
    invite(b, token, plain, puppet)  # what an Element user does first
    invite(b, token, plain, concierge)
    log(f"{me} invited {puppet} and then {concierge}")
    asked, since = wait_for(b, token, lambda s: [n for n in notices_from(s, dm, concierge) if plain in n and ("moderator" in n or "Added" in n or "could not" in n or "did not" in n)], 180, since)
    if not asked:
        sys.exit("FAIL: the concierge said nothing about the remote room")
    log(f"concierge after {time.time()-t1:.0f}s: {asked[-1][:160]}")
    if "moderator" in asked[-1]:
        rooms.grant_moderator(al, host_token, plain, concierge)
        log("alpha room creator granted moderator to the concierge")
        asked, since = wait_for(b, token, lambda s: [n for n in notices_from(s, dm, concierge) if "Added" in n and plain in n or (plain in n and "did not" in n)], 180, since)
    if not asked or "Added" not in asked[-1]:
        sys.exit(f"FAIL: the concierge did not add the Agent: {asked[-1] if asked else 'no reply'}")
    log(f"confirmation after {time.time()-t1:.0f}s")
    joined, _ = wait_for(al, host_token, lambda s: member_is(s, plain, puppet, "join"), 120)
    if not joined:
        sys.exit("FAIL: the Agent did not join the remote room")
    rooms.send(al, host_token, plain, f"{puppet}: Reply with exactly one word: pong", puppet)
    placeholder = lambda t: "request received" in t.lower() or "请求已收到" in t
    answer, _ = wait_for(al, host_token, lambda s: [n for n in notices_from(s, plain, puppet) if not placeholder(n)], 300)
    if not answer:
        sys.exit("FAIL: the Agent did not answer in the remote room")
    log(f"agent answered an alpha user in the alpha room after {time.time()-t1:.0f}s: {answer[-1][:80]}")
    print("PASS")

if __name__ == "__main__":
    main()
