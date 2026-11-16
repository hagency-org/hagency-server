#!/usr/bin/env python3
"""Room context end to end (client API; needs the server and the Agent runtime
relaunched with room context): a fresh user gets a hosted Agent from the
concierge, adds it to a plain room (as demo-concierge-rooms-e2e.py does),
posts distinctive messages there, then asks the Agent
to summarize the room. PASS only if the reply repeats a phrase that appeared
ONLY in the earlier messages (never in the request). The same is checked
inside a Matrix thread. Empty, placeholder or "cannot see" replies FAIL.

usage: demo-room-context-e2e.py --user NEWNAME [--server http://127.0.0.1:8092]
       [--server-name localhost:8452] [--concierge hagency]
"""
import argparse, importlib.util, os, sys, time, urllib.parse

spec = importlib.util.spec_from_file_location("beat7", os.path.join(os.path.dirname(__file__), "demo-concierge-e2e.py"))
beat7 = importlib.util.module_from_spec(spec); spec.loader.exec_module(beat7)
call, log, wait_for, notices_from = beat7.call, beat7.log, beat7.wait_for, beat7.notices_from
spec = importlib.util.spec_from_file_location("rooms", os.path.join(os.path.dirname(__file__), "demo-concierge-rooms-e2e.py"))
rooms = importlib.util.module_from_spec(spec); spec.loader.exec_module(rooms)

ROOM_FACTS = ["The launch codename is BLUE HERON.", "Budget review moved to the Lisbon office.", "Nadia owns the PELICAN-7 checklist."]
ROOM_PHRASES = ["blue heron", "lisbon", "pelican-7"]
THREAD_ROOT = "Thread: vendor shortlist for the kiosk project."
THREAD_FACT = "The preferred vendor in this thread is MARIGOLD SYSTEMS."
THREAD_PHRASES = ["marigold"]

def send(origin, token, room, text, mention=None, thread=None):
    body = {"msgtype": "m.text", "body": text}
    if mention:
        body["m.mentions"] = {"user_ids": [mention]}
    if thread:
        body["m.relates_to"] = {"rel_type": "m.thread", "event_id": thread, "is_falling_back": True, "m.in_reply_to": {"event_id": thread}}
    status, out = call("PUT", f"{origin}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/send/m.room.message/c{time.time_ns()}", body, token)
    return out.get("event_id") or sys.exit(f"send: {status} {out}")

def is_answer(text):
    t = text.lower()
    return bool(t.strip()) and "request received" not in t and "请求已收到" not in text

def agent_in_plain_room(o, token, dm, concierge):
    """Same flow as demo-concierge-rooms-e2e.py. A plain room (owner, concierge,
    Agent) requires a mention, so the facts posted below never trigger the
    Agent; its only route to them is the room context (R14: no session memory)."""
    send(o, token, dm, "create agent")
    created, since = wait_for(o, token, lambda s: [n for n in notices_from(s, dm, concierge) if "Created" in n], 240)
    if not created:
        sys.exit("FAIL: no Agent created")
    puppet = created[-1].split("(")[1].split(")")[0]
    plain = rooms.create_room(o, token, "e2e room context", [concierge], False)
    asked, since = wait_for(o, token, lambda s: [n for n in notices_from(s, dm, concierge) if plain in n and ("moderator" in n or "Added" in n)], 180, since)
    if not asked:
        sys.exit("FAIL: the concierge neither added the Agent nor asked for room power")
    if "Added" not in asked[-1]:
        rooms.grant_moderator(o, token, plain, concierge)
        added, since = wait_for(o, token, lambda s: [n for n in notices_from(s, dm, concierge) if "Added" in n and plain in n], 180, since)
        if not added:
            sys.exit("FAIL: the concierge did not confirm adding the Agent")
    joined, since = wait_for(o, token, lambda s: any(ev.get("state_key") == puppet and ev["content"].get("membership") == "join"
                       for part in ("state", "timeline") for ev in s.get("rooms", {}).get("join", {}).get(plain, {}).get(part, {}).get("events", [])), 120)
    if not joined:
        sys.exit("FAIL: the Agent did not join the plain room")
    return plain, puppet, since

def ask(o, token, room, puppet, since, request, phrases, thread=None, label="room"):
    t = time.time()
    send(o, token, room, f"{puppet}: {request}", puppet, thread)
    answer, since = wait_for(o, token, lambda s: [n for n in notices_from(s, room, puppet) if is_answer(n)], 300, since)
    if not answer:
        sys.exit(f"FAIL ({label}): the Agent did not answer (only a placeholder, or nothing)")
    text = answer[-1]
    log(f"{label} answer after {time.time()-t:.0f}s: {text[:160]!r}")
    if not any(p in text.lower() for p in phrases):
        sys.exit(f"FAIL ({label}): the reply quotes none of {phrases}; the Agent did not see the earlier messages")
    return since

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
    status, body = call("POST", f"{o}/_matrix/client/v3/createRoom", {"is_direct": True, "preset": "trusted_private_chat", "invite": [concierge]}, token)
    dm = body.get("room_id") or sys.exit(f"createRoom: {status} {body}")
    wait_for(o, token, lambda s: any(ev.get("state_key") == concierge and ev["content"].get("membership") == "join"
             for part in ("state", "timeline") for ev in s.get("rooms", {}).get("join", {}).get(dm, {}).get(part, {}).get("events", [])), 60)
    room, puppet, since = agent_in_plain_room(o, token, dm, concierge)
    log(f"{me}: Agent {puppet} in {room}")
    for fact in ROOM_FACTS:
        send(o, token, room, fact)
    # Unmentioned facts must not be answered; an answer here would also give
    # the Agent the facts through its own session (vacuous pass).
    early, since = wait_for(o, token, lambda s: [n for n in notices_from(s, room, puppet) if is_answer(n)], 20, since)
    if early:
        sys.exit(f"FAIL: the Agent answered an unmentioned message: {early[-1][:80]!r}")
    since = ask(o, token, room, puppet, since, "summarize this room so far", ROOM_PHRASES)
    root = send(o, token, room, THREAD_ROOT)
    send(o, token, room, THREAD_FACT, thread=root)
    ask(o, token, room, puppet, since, "summarize this thread so far", THREAD_PHRASES, thread=root, label="thread")
    print("PASS")

if __name__ == "__main__":
    main()
