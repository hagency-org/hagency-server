#!/usr/bin/env python3
"""Two owners on the SAME server bind their Agents into one plain room hosted on
another server (ADR 0001, scene 6 on haws). On beta: owner A runs
`hagency agent up --runtime claude` headless; user W gets a hosted Agent from the
concierge. An alpha user creates the room and invites both; neither gets room
power. A invites its puppet, W invites the concierge. PASS only if both Agents
join and both answer the alpha user. Before ADR 0001 the second owner was
refused (room_already_registered / domain_object_not_authorized) and every owner
needed moderator power. Then "Agents go with their owners": the alpha user
kicks owner A; A's Agent must leave on its own; after A rejoins it must NOT come
back until A adds it again, and then it answers. Cleanup stops `agent up` and
scrubs the three users.

usage: demo-two-owners-one-room-e2e.py --user zztoNNNN --home DIR [--bin PATH]
"""
import argparse, importlib.util, os, shutil, subprocess, sys, time, urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
def load(name, file):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, file))
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module); return module
claude = load("claude_ctx", "demo-claude-remote-context-e2e.py")
remote = load("remote_room", "demo-concierge-remote-room-e2e.py")
beat7, rooms = claude.beat7, remote.rooms
call, log, wait_for, notices_from = beat7.call, beat7.log, beat7.wait_for, beat7.notices_from
ALPHA, ALPHA_NAME, BETA, BETA_NAME = claude.ALPHA, claude.ALPHA_NAME, claude.BETA, claude.BETA_NAME
CONCIERGE = f"@hagency:{BETA_NAME}"

def password(user):
    return f"Demo-{user}-2026-pass"

def join_remote(token, room):
    for _ in range(30):
        status, body = call("POST", f"{BETA}/_matrix/client/v3/join/{urllib.parse.quote(room, safe='')}?server_name={ALPHA_NAME}", {}, token)
        if status == 200:
            return
        time.sleep(1)
    sys.exit(f"FAIL: join {room}: {status} {body}")

def answers(token, room, puppet, since):
    placeholder = lambda t: "request received" in t.lower() or "请求已收到" in t
    rooms.send(ALPHA, token, room, f"{puppet}: Reply with exactly one word: pong", puppet)
    found, _ = wait_for(ALPHA, token, lambda s: [n for n in notices_from(s, room, puppet) if not placeholder(n)], 300, since)
    return found

def run(a, timings):
    owner, web, host = f"{a.user}-a", f"{a.user}-w", f"{a.user}-h"
    for user, server in ((owner, BETA), (web, BETA), (host, ALPHA)):
        beat7.register(server, user, password(user))
    tok_a, me_a = beat7.matrix_login(BETA, owner, password(owner))
    tok_w, me_w = beat7.matrix_login(BETA, web, password(web))
    tok_h, me_h = beat7.matrix_login(ALPHA, host, password(host))
    t = time.time()
    a.proc, puppet_a, log_path = claude.start_agent(a.bin, a.home, owner, password(owner), BETA, BETA_NAME)
    timings["agent_up_ready"] = time.time() - t
    dm = rooms.create_room(BETA, tok_w, "concierge", [CONCIERGE], False)
    wait_for(BETA, tok_w, lambda s: remote.member_is(s, dm, CONCIERGE, "join"), 60)
    rooms.send(BETA, tok_w, dm, "create agent")
    created, since_w = wait_for(BETA, tok_w, lambda s: [n for n in notices_from(s, dm, CONCIERGE) if "Created" in n], 240)
    if not created:
        sys.exit("FAIL: no hosted Agent created")
    puppet_w = created[-1].split("(")[1].split(")")[0]
    log(f"owner A {me_a} → {puppet_a}; hosted {puppet_w} for {me_w}")

    room = rooms.create_room(ALPHA, tok_h, "e2e two owners", [me_a, me_w], False)
    join_remote(tok_a, room); join_remote(tok_w, room)
    log(f"{me_h} created {room} on alpha; both beta users joined without room power")
    t = time.time()
    remote.invite(BETA, tok_a, room, puppet_a)
    remote.invite(BETA, tok_w, room, CONCIERGE)
    for puppet in (puppet_a, puppet_w):
        ok, _ = wait_for(ALPHA, tok_h, lambda s, p=puppet: remote.member_is(s, room, p, "join"), 180)
        if not ok:
            notes, _ = wait_for(BETA, tok_w, lambda s: notices_from(s, dm, CONCIERGE), 5, since_w)
            sys.exit(f"FAIL: {puppet} did not join; concierge said {notes}; agent up log:\n{open(log_path).read()[-1200:]}")
    timings["both_joined"] = time.time() - t
    log(f"both Agents joined after {timings['both_joined']:.0f}s")
    _, since_h = wait_for(ALPHA, tok_h, lambda s: True, 1)
    for label, puppet in (("agent up", puppet_a), ("hosted", puppet_w)):
        t = time.time()
        reply = answers(tok_h, room, puppet, since_h)
        if not reply:
            sys.exit(f"FAIL: the {label} Agent {puppet} did not answer")
        timings[f"{label}_reply"] = time.time() - t
        log(f"{label} Agent answered in {timings[f'{label}_reply']:.0f}s: {reply[-1][:60]}")
    follow_owner(tok_h, tok_a, me_a, room, puppet_a, timings)
    # The hosted Agent goes with the person it was made for (the concierge owns it).
    t = time.time()
    status, body = call("POST", f"{ALPHA}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/kick", {"user_id": me_w, "reason": "e2e"}, tok_h)
    if status != 200:
        sys.exit(f"FAIL: kick {me_w}: {status} {body}")
    if not wait_membership(tok_h, room, puppet_w, "leave", 60):
        sys.exit(f"FAIL: hosted {puppet_w} stayed after its requester was kicked")
    timings["hosted_left_after_kick"] = time.time() - t
    log(f"requester kicked; the hosted Agent left after {timings['hosted_left_after_kick']:.0f}s")

def membership(token, room, user):
    status, body = call("GET", f"{ALPHA}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/state/m.room.member/{urllib.parse.quote(user, safe='')}", token=token)
    return body.get("membership") if status == 200 else None

def wait_membership(token, room, user, wanted, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if membership(token, room, user) == wanted:
            return True
        time.sleep(1)
    return False

def follow_owner(tok_h, tok_a, me_a, room, puppet, timings):
    """Owner kicked → Agent leaves; rejoin → stays out until re-added."""
    t = time.time()
    status, body = call("POST", f"{ALPHA}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/kick", {"user_id": me_a, "reason": "e2e"}, tok_h)
    if status != 200:
        sys.exit(f"FAIL: kick {me_a}: {status} {body}")
    if not wait_membership(tok_h, room, puppet, "leave", 60):
        sys.exit(f"FAIL: {puppet} stayed after its owner was kicked (membership {membership(tok_h, room, puppet)})")
    timings["agent_left_after_kick"] = time.time() - t
    log(f"owner kicked; their Agent left after {timings['agent_left_after_kick']:.0f}s")
    remote.invite(ALPHA, tok_h, room, me_a)
    join_remote(tok_a, room)
    time.sleep(20)  # longer than ten membership sweeps: nothing may re-add it
    if membership(tok_h, room, puppet) == "join":
        sys.exit("FAIL: the Agent came back by itself when its owner rejoined")
    log("owner rejoined; the Agent stayed out (as required)")
    t = time.time()
    remote.invite(BETA, tok_a, room, puppet)
    if not wait_membership(tok_h, room, puppet, "join", 180):
        sys.exit("FAIL: the re-added Agent did not join")
    _, since = wait_for(ALPHA, tok_h, lambda s: True, 1)
    reply = answers(tok_h, room, puppet, since)
    if not reply:
        sys.exit("FAIL: the re-added Agent did not answer")
    timings["re_add_answer"] = time.time() - t
    log(f"re-added Agent answered after {timings['re_add_answer']:.0f}s: {reply[-1][:60]}")

def cleanup(a):
    if getattr(a, "proc", None) and a.proc.poll() is None:
        a.proc.terminate()
        try:
            a.proc.wait(15)
        except subprocess.TimeoutExpired:
            a.proc.kill()
        log(f"stopped agent up pid {a.proc.pid}")
    short = os.path.join(os.path.realpath(claude.SHORT_LINK_DIR), f"{a.user}-a")
    if os.path.isdir(short):
        shutil.rmtree(short)
    for user, inst in ((f"{a.user}-a", "beta"), (f"{a.user}-w", "beta"), (f"{a.user}-h", "alpha")):
        r = subprocess.run(["bash", os.path.join(HERE, "demo-scrub-user.sh"), user, inst], capture_output=True, text=True)
        log(f"scrub {user}@{inst}: rc={r.returncode}")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--user", required=True, help="a NEW base name; -a, -w (beta) and -h (alpha) are appended")
    ap.add_argument("--home", required=True, help="fresh directory for the agent-up home, password file and log")
    ap.add_argument("--bin", default=claude.DEFAULT_BIN)
    a = ap.parse_args()
    if os.path.exists(a.home):
        sys.exit(f"{a.home} exists; use a fresh --home and a NEW --user")
    timings = {}
    try:
        run(a, timings)
        print("PASS", {k: f"{v:.0f}s" for k, v in timings.items()})
    finally:
        cleanup(a)

if __name__ == "__main__":
    main()
