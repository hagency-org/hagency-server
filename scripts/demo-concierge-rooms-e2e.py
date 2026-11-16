#!/usr/bin/env python3
"""Concierge room invites end to end (client API, after the concierge relaunch):
a fresh user gets a hosted Agent, then invites the concierge into a plain room
of their own; the concierge must bring the Agent, confirm in the user's chat,
and the Agent must answer there. An encrypted room must be declined with an
explanation. Exits non-zero on any missing step; empty replies are failures.

usage: demo-concierge-rooms-e2e.py --user NEWNAME [--server http://127.0.0.1:8092]
       [--server-name localhost:8452] [--concierge hagency]
"""
import argparse, importlib.util, json, os, sys, time, urllib.parse

spec = importlib.util.spec_from_file_location("beat7", os.path.join(os.path.dirname(__file__), "demo-concierge-e2e.py"))
beat7 = importlib.util.module_from_spec(spec); spec.loader.exec_module(beat7)
call, log, wait_for, notices_from = beat7.call, beat7.log, beat7.wait_for, beat7.notices_from

def send(origin, token, room, text, mention=None):
    body = {"msgtype": "m.text", "body": text}
    if mention:
        body["m.mentions"] = {"user_ids": [mention]}
    call("PUT", f"{origin}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/send/m.room.message/t{time.time_ns()}", body, token)

def create_room(origin, token, name, invite, encrypted):
    state = [{"type": "m.room.encryption", "state_key": "", "content": {"algorithm": "m.megolm.v1.aes-sha2"}}] if encrypted else []
    status, body = call("POST", f"{origin}/_matrix/client/v3/createRoom", {"name": name, "preset": "private_chat", "invite": invite, "initial_state": state}, token)
    return body.get("room_id") or sys.exit(f"createRoom {name}: {status} {body}")

def grant_moderator(origin, token, room, user):
    path = f"{origin}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/state/m.room.power_levels/"
    status, levels = call("GET", path, token=token)
    if status != 200:
        sys.exit(f"read power levels: {status} {levels}")
    levels.setdefault("users", {})[user] = 50
    status, body = call("PUT", path, levels, token)
    if status != 200:
        sys.exit(f"grant moderator: {status} {body}")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8092")
    ap.add_argument("--server-name", default="localhost:8452")
    ap.add_argument("--concierge", default="hagency")
    ap.add_argument("--user", required=True, help="a NEW username (one hosted Agent per user)")
    a = ap.parse_args()
    o, password, concierge = a.server, f"Demo-{a.user}-2026-pass", f"@{a.concierge}:{a.server_name}"
    beat7.register(o, a.user, password)
    token, me = beat7.matrix_login(o, a.user, password)
    t0 = time.time()
    dm = create_room(o, token, "concierge", [concierge], False)
    wait_for(o, token, lambda s: any(ev.get("state_key") == concierge and ev["content"].get("membership") == "join"
             for part in ("state", "timeline") for ev in s.get("rooms", {}).get("join", {}).get(dm, {}).get(part, {}).get("events", [])), 60)
    send(o, token, dm, "create agent")
    created, since = wait_for(o, token, lambda s: [n for n in notices_from(s, dm, concierge) if "Created" in n], 240)
    if not created:
        sys.exit("FAIL: no Agent created")
    puppet = created[-1].split("(")[1].split(")")[0]
    log(f"created {puppet} in {time.time()-t0:.0f}s")

    plain = create_room(o, token, "e2e plain", [concierge], False)
    t1 = time.time()
    # In a room the requester created, the concierge has no power yet: the
    # server refuses the bind and the concierge asks for moderator once
    # (17:42Z failure: it gave up silently). Grant it like Element would.
    asked, since = wait_for(o, token, lambda s: [n for n in notices_from(s, dm, concierge) if plain in n and ("moderator" in n or "Added" in n)], 180, since)
    if not asked:
        sys.exit("FAIL: the concierge neither added the Agent nor asked for room power")
    if "moderator" in asked[-1]:
        log(f"concierge asked for power after {time.time()-t1:.0f}s")
        grant_moderator(o, token, plain, concierge)
        log("granted moderator to the concierge")
    added, since = wait_for(o, token, lambda s: [n for n in notices_from(s, dm, concierge) if "Added" in n and plain in n], 180, since) if "Added" not in asked[-1] else (asked, since)
    if not added:
        sys.exit("FAIL: the concierge did not confirm adding the Agent to the plain room")
    log(f"confirmation after {time.time()-t1:.0f}s: {added[-1]}")
    joined, _ = wait_for(o, token, lambda s: any(ev.get("state_key") == puppet and ev["content"].get("membership") == "join"
                        for part in ("state", "timeline") for ev in s.get("rooms", {}).get("join", {}).get(plain, {}).get(part, {}).get("events", [])), 120)
    if not joined:
        sys.exit("FAIL: the Agent did not join the plain room")
    send(o, token, plain, f"{puppet}: Reply with exactly one word: pong", puppet)
    placeholder = lambda t: "request received" in t.lower() or "请求已收到" in t
    answer, _ = wait_for(o, token, lambda s: [n for n in notices_from(s, plain, puppet) if not placeholder(n)], 300)
    if not answer:
        sys.exit("FAIL: the Agent did not answer in the plain room")
    log(f"agent answered in the plain room after {time.time()-t1:.0f}s: {answer[-1][:80]}")

    secret = create_room(o, token, "e2e encrypted", [concierge], True)
    refused, _ = wait_for(o, token, lambda s: [n for n in notices_from(s, dm, concierge) if "encryption" in n], 120, since)
    if not refused:
        sys.exit("FAIL: no refusal for the encrypted room")
    log(f"encrypted room {secret} declined: {refused[-1][:90]}")
    print("PASS")

if __name__ == "__main__":
    main()
