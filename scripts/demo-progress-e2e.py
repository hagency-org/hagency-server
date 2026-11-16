#!/usr/bin/env python3
"""Live progress end to end over the Matrix client API (needs the server and the
Agent runtime rebuilt with live progress): a fresh user gets a hosted Agent from
the concierge, asks it something that takes several tool steps, and checks the
Agent's status message in the request's thread:
  1. one m.notice "⏳ Working…" from the Agent's puppet appears,
  2. it is edited (m.replace, same sender) at least twice,
  3. its last edit reads "✅ Done in N s · M steps",
  4. the generic "Still working…" notice is not sent,
  5. a real answer (not a status or placeholder) arrives.
Exits non-zero on any miss; no status, no edits or an empty answer FAIL (R14).

usage: demo-progress-e2e.py --user NEWNAME [--server http://127.0.0.1:8092]
       [--server-name localhost:8452] [--concierge hagency] [--password-file FILE]
"""
import argparse, importlib.util, json, os, sys, time, urllib.parse

spec = importlib.util.spec_from_file_location("beat7", os.path.join(os.path.dirname(__file__), "demo-concierge-e2e.py"))
beat7 = importlib.util.module_from_spec(spec); spec.loader.exec_module(beat7)
call, log, wait_for, notices_from = beat7.call, beat7.log, beat7.wait_for, beat7.notices_from

QUESTION = ("Please do this step by step with your tools: list the files in your workspace, "
            "create a file notes.md with three short lines about Matrix, read notes.md back, "
            "then tell me how many lines it has.")

def put(o, token, room, body, tag):
    status, out = call("PUT", f"{o}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/send/m.room.message/{tag}{time.time_ns()}", body, token)
    return out.get("event_id") or sys.exit(f"send: {status} {out}")

def agent_room(a, token):
    """DM the concierge "create agent"; return (room, puppet) of the hosted Agent."""
    concierge = f"@{a.concierge}:{a.server_name}"
    status, body = call("POST", f"{a.server}/_matrix/client/v3/createRoom", {"is_direct": True, "preset": "trusted_private_chat", "invite": [concierge]}, token)
    dm = body.get("room_id") or sys.exit(f"createRoom: {status} {body}")
    joined = lambda s: any(ev.get("state_key") == concierge and ev["content"].get("membership") == "join"
                           for ev in s.get("rooms", {}).get("join", {}).get(dm, {}).get("timeline", {}).get("events", [])) or concierge in json.dumps(s.get("rooms", {}).get("join", {}).get(dm, {}).get("state", {}))
    _, since = wait_for(a.server, token, joined, 60)
    put(a.server, token, dm, {"msgtype": "m.text", "body": "create agent"}, "ca")
    reply, since = wait_for(a.server, token, lambda s: notices_from(s, dm, concierge), 240, since)
    if not reply or "Created" not in reply[-1]:
        sys.exit(f"FAIL: the concierge did not create an Agent: {reply[-1] if reply else 'no reply'}")
    invites, _ = wait_for(a.server, token, lambda s: list(s.get("rooms", {}).get("invite", {}).keys()), 60, None)
    if not invites:
        sys.exit("FAIL: no room invite arrived")
    room = invites[0]
    call("POST", f"{a.server}/_matrix/client/v3/join/{urllib.parse.quote(room, safe='')}", {}, token)
    def find_puppet(s):
        r = s.get("rooms", {}).get("join", {}).get(room, {})
        for ev in r.get("state", {}).get("events", []) + r.get("timeline", {}).get("events", []):
            if ev.get("type") == "m.room.member" and ev.get("state_key", "").startswith("@_hagency_agt_") and ev["content"].get("membership") == "join":
                return ev["state_key"]
    puppet, _ = wait_for(a.server, token, find_puppet, 120, None)
    return room, puppet or sys.exit("FAIL: the Agent puppet did not join the room")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8092")
    ap.add_argument("--server-name", default="localhost:8452")
    ap.add_argument("--concierge", default="hagency")
    ap.add_argument("--user", required=True, help="a NEW username per run (one hosted Agent per user)")
    ap.add_argument("--password-file", default=None)
    a = ap.parse_args()
    password = open(a.password_file).read().strip() if a.password_file else f"Demo-{a.user}-2026-pass"
    beat7.register(a.server, a.user, password)
    token, me = beat7.matrix_login(a.server, a.user, password)
    room, puppet = agent_room(a, token)
    log(f"agent {puppet} in {room}")
    # Settle the timeline so only events after the question are inspected.
    _, since = wait_for(a.server, token, lambda s: True, 5)
    asked_at = time.time()
    put(a.server, token, room, {"msgtype": "m.text", "body": f"{puppet}: {QUESTION}", "m.mentions": {"user_ids": [puppet]}}, "pq")
    seen = []  # every m.room.message from the puppet, in order
    def collect(s):
        for ev in s.get("rooms", {}).get("join", {}).get(room, {}).get("timeline", {}).get("events", []):
            if ev.get("type") == "m.room.message" and ev.get("sender") == puppet:
                seen.append(ev)
        status = next((e for e in seen if e["content"].get("body", "").startswith("⏳ Working…")), None)
        if not status:
            return None
        edits = [e for e in seen if e["content"].get("m.relates_to", {}).get("rel_type") == "m.replace"
                 and e["content"]["m.relates_to"].get("event_id") == status["event_id"]]
        done = edits and edits[-1]["content"].get("m.new_content", {}).get("body", "").startswith("✅ Done in")
        answers = [e for e in seen if e is not status and e not in edits and e["content"].get("msgtype") == "m.text" and e["content"].get("body", "").strip()]
        return (status, edits, answers) if done and answers else None
    found, _ = wait_for(a.server, token, collect, 600, since)
    if not found:
        status = next((e for e in seen if e["content"].get("body", "").startswith("⏳ Working…")), None)
        sys.exit("FAIL: " + ("no \"Working…\" status from the Agent" if not status else
                 f"status never reached \"Done in\" with an answer; saw {len(seen)} puppet events"))
    status, edits, answers = found
    first_delay = status.get("origin_server_ts", 0) / 1000 - asked_at
    log(f"status {status['event_id']} posted ~{first_delay:.1f} s after the question")
    for e in edits:
        log("edit: " + e["content"]["m.new_content"]["body"].splitlines()[0][:100])
    if len(edits) < 2:
        sys.exit(f"FAIL: the status was edited {len(edits)} time(s); expected at least 2")
    stamps = [e.get("origin_server_ts", 0) for e in edits]
    if any(b - a < 1400 for a, b in zip(stamps, stamps[1:])):
        sys.exit("FAIL: two status edits less than 1.5 s apart (throttle)")
    if any("still working" in e["content"].get("body", "").lower() for e in seen):
        sys.exit("FAIL: the generic \"Still working…\" notice was sent while progress was active")
    log(f"answer: {answers[-1]['content']['body'][:120]}")
    print("PASS")

if __name__ == "__main__":
    main()
