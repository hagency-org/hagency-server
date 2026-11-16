#!/usr/bin/env python3
"""Room context across federation (client API): a headless CLAUDE Agent owned
on alpha summarizes a plain room hosted on beta. A fresh alpha user starts
`hagency agent up --runtime claude` (no trusted tools); a fresh beta user
creates the room, invites the owner, raises it to power 50 (an owner may only
register a room whose state it can manage), and the owner invites its
puppet. The beta user posts facts unmentioned, then mentions the Agent:
PASS only if the reply repeats a fact (R14: the facts never reach the Agent
through its own session). Cleanup stops the started `agent up` and scrubs
both users; the Agent record stays (permanent by design).

usage: demo-claude-remote-context-e2e.py --user zzrcNNNN --home DIR [--bin PATH]
"""
import argparse, importlib.util, os, re, shutil, subprocess, sys, time, urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("beat7", os.path.join(HERE, "demo-concierge-e2e.py"))
beat7 = importlib.util.module_from_spec(spec); spec.loader.exec_module(beat7)
call, log, wait_for, notices_from = beat7.call, beat7.log, beat7.wait_for, beat7.notices_from
spec = importlib.util.spec_from_file_location("ctx", os.path.join(HERE, "demo-room-context-e2e.py"))
ctx = importlib.util.module_from_spec(spec); spec.loader.exec_module(ctx)
rooms = ctx.rooms

ALPHA, ALPHA_NAME = "http://127.0.0.1:8090", "localhost:8450"
BETA, BETA_NAME = "http://127.0.0.1:8092", "localhost:8452"
DEFAULT_BIN = os.path.join(HERE, "..", "..", "hagency-client", "target", "demo", "hagency")
READY_TIMEOUT_S, JOIN_TIMEOUT_S, QUIET_WINDOW_S = 240, 180, 20
# macOS caps a Unix socket path (sun_path) at 104 bytes; the daemon binds
# <home>/control.sock and, past that, fails with only "the owner daemon did
# not start". The daemon refuses a symlinked home (daemon_socket_unsafe), so
# a long --home gets its agent home in the gitignored hagency-server/.run/e2e.
SUN_PATH_MAX, SHORT_LINK_DIR = 100, os.path.join(HERE, "..", ".run", "e2e")

def joined(room, user):
    return lambda s: any(ev.get("state_key") == user and ev["content"].get("membership") == "join"
                         for part in ("state", "timeline") for ev in s.get("rooms", {}).get("join", {}).get(room, {}).get(part, {}).get("events", []))

def start_agent(binary, home, user, password, server=ALPHA, server_name=ALPHA_NAME):
    os.makedirs(home, exist_ok=True)
    pw_file, log_path = os.path.join(home, "password"), os.path.join(home, "agent-up.log")
    with open(os.open(pw_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as f:
        f.write(password + "\n")
    out = open(log_path, "w")
    agent_home = os.path.join(home, "agent-home")
    if len(os.path.join(agent_home, "control.sock")) > SUN_PATH_MAX:
        agent_home = os.path.join(os.path.realpath(SHORT_LINK_DIR), user)
        log(f"agent home {agent_home} (the --home path is too long for its socket)")
    os.makedirs(agent_home, mode=0o700)
    proc = subprocess.Popen([binary, "agent", "up", "--home", agent_home, "--server", server,
                             "--server-name", server_name, "--username", user, "--password-file", pw_file,
                             "--name", f"{user}-claude", "--runtime", "claude"],
                            stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, env={**os.environ, "RUST_LOG": "warn"})
    log(f"agent up pid {proc.pid}, log {log_path}")
    deadline = time.time() + READY_TIMEOUT_S
    while time.time() < deadline:
        text = open(log_path).read()
        m = re.search(r"hosting .*; invite (@\S+) to a room", text)
        if m:
            return proc, m.group(1), log_path
        # matrix_sdk logs benign 404s as "ERROR ... Error while sending"; only
        # a top-level "Error:" line (or exit) means agent up failed.
        if proc.poll() is not None or re.search(r"^Error", text, re.M):
            break
        time.sleep(2)
    proc.terminate()
    sys.exit(f"FAIL: agent up not ready:\n{open(log_path).read()[-2000:]}")

def run(a, timings):
    owner, creator = f"{a.user}-a", f"{a.user}-b"
    pw_a, pw_b = f"Demo-{owner}-2026-pass", f"Demo-{creator}-2026-pass"
    t0 = time.time()
    beat7.register(ALPHA, owner, pw_a)
    beat7.register(BETA, creator, pw_b)
    tok_a, me_a = beat7.matrix_login(ALPHA, owner, pw_a)
    tok_b, me_b = beat7.matrix_login(BETA, creator, pw_b)
    timings["register"] = time.time() - t0
    t = time.time()
    a.proc, puppet, log_path = start_agent(a.bin, a.home, owner, pw_a)
    timings["agent_ready"] = time.time() - t
    log(f"owner {me_a}, Agent {puppet}; room creator {me_b}")
    t = time.time()
    room = rooms.create_room(BETA, tok_b, "e2e remote claude context", [me_a], False)
    status, body = call("POST", f"{ALPHA}/_matrix/client/v3/join/{urllib.parse.quote(room, safe='')}?server_name={BETA_NAME}", {}, tok_a)
    if status != 200:
        sys.exit(f"FAIL: owner join over federation: {status} {body}")
    rooms.grant_moderator(BETA, tok_b, room, me_a)
    ok, since_a = wait_for(ALPHA, tok_a, lambda s: s.get("rooms", {}).get("join", {}).get(room), 30)
    status, body = call("POST", f"{ALPHA}/_matrix/client/v3/rooms/{urllib.parse.quote(room, safe='')}/invite", {"user_id": puppet}, tok_a)
    if status != 200:
        sys.exit(f"FAIL: owner invite puppet: {status} {body}")
    ok, since_b = wait_for(BETA, tok_b, joined(room, puppet), JOIN_TIMEOUT_S)
    if not ok:
        sys.exit(f"FAIL: the puppet never joined {room}; agent up log:\n{open(log_path).read()[-1500:]}")
    timings["puppet_join"] = time.time() - t
    log(f"puppet joined {room} after {timings['puppet_join']:.0f}s")
    for fact in ctx.ROOM_FACTS:
        ctx.send(BETA, tok_b, room, fact)
    early, since_b = wait_for(BETA, tok_b, lambda s: [n for n in notices_from(s, room, puppet) if ctx.is_answer(n)], QUIET_WINDOW_S, since_b)
    if early:
        sys.exit(f"FAIL: the Agent answered an unmentioned message: {early[-1][:80]!r}")
    t = time.time()
    ctx.send(BETA, tok_b, room, f"{puppet}: summarize this room so far", puppet)
    answer, _ = wait_for(BETA, tok_b, lambda s: [n for n in notices_from(s, room, puppet) if ctx.is_answer(n)], 300, since_b)
    timings["reply"] = time.time() - t
    if not answer:
        sys.exit(f"FAIL: no answer (only a placeholder, or nothing) after {timings['reply']:.0f}s; log:\n{open(log_path).read()[-1500:]}")
    text = answer[-1]
    print(f"REPLY: {text}")
    hits = [p for p in ctx.ROOM_PHRASES if p in text.lower()]
    if not hits:
        sys.exit(f"FAIL: the reply quotes none of {ctx.ROOM_PHRASES}")
    log(f"facts quoted: {hits}")

def cleanup(a):
    if getattr(a, "proc", None) and a.proc.poll() is None:
        a.proc.terminate()
        try:
            a.proc.wait(15)
        except subprocess.TimeoutExpired:
            a.proc.kill()
        log(f"stopped agent up pid {a.proc.pid}")
    short = os.path.join(os.path.realpath(SHORT_LINK_DIR), f"{a.user}-a")
    if os.path.isdir(short):
        shutil.rmtree(short)  # its owner is scrubbed below; the home is useless after
    if a.keep_users:
        return
    for user, inst in ((f"{a.user}-a", "alpha"), (f"{a.user}-b", "beta")):
        r = subprocess.run(["bash", os.path.join(HERE, "demo-scrub-user.sh"), user, inst], capture_output=True, text=True)
        log(f"scrub {user}@{inst}: rc={r.returncode} {(r.stdout + r.stderr).strip().splitlines()[-1:]}")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--user", required=True, help="a NEW base name; -a (alpha owner) and -b (beta creator) are appended")
    ap.add_argument("--home", required=True, help="fresh directory for the agent home, password file and log")
    ap.add_argument("--bin", default=DEFAULT_BIN)
    ap.add_argument("--keep-users", action="store_true")
    a = ap.parse_args()
    if os.path.exists(a.home):
        sys.exit(f"{a.home} exists; use a fresh --home and a NEW --user")
    timings = {}
    try:
        run(a, timings)
        print("PASS", {k: f"{v:.0f}s" for k, v in timings.items()})
    finally:
        print("timings", {k: f"{v:.0f}s" for k, v in timings.items()}, flush=True)
        cleanup(a)

if __name__ == "__main__":
    main()
