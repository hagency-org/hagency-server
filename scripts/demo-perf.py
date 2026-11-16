#!/usr/bin/env python3
"""Headless latency table for the local two-server demo (Phase 3 perf pass).

Measures, with FRESH throwaway users (zzperf<tag>a/b/c, never demo users):
  1. alpha->beta and beta->alpha message visible latency (send -> peer /sync)
  2. same-server baseline (two users on beta)
  3. concierge "create agent" time and first Agent reply (optional, --concierge-runs)
  5. federation /version: direct http vs the TLS relay, new connection vs keep-alive

Prints the users it created so they can be scrubbed afterwards with
scripts/demo-scrub-user.sh. Exits non-zero when a step fails (R14: a missing
message is a failure, never a fast sample).

usage: demo-perf.py --tag NNNN [-n 20] [--concierge-runs 0] [--fed-n 50] [--only msg,fed,concierge]
"""
import argparse, http.client, importlib.util, os, ssl, statistics, sys, threading, time, urllib.parse

spec = importlib.util.spec_from_file_location("beat7", os.path.join(os.path.dirname(__file__), "demo-concierge-e2e.py"))
beat7 = importlib.util.module_from_spec(spec); spec.loader.exec_module(beat7)
call, log, wait_for, notices_from = beat7.call, beat7.log, beat7.wait_for, beat7.notices_from

ALPHA = ("http://127.0.0.1:8090", "localhost:8450")
BETA = ("http://127.0.0.1:8092", "localhost:8452")
SEND_GAP_S = 0.5          # spacing between timed messages so each is its own transaction
ARRIVAL_TIMEOUT_S = 60    # a message not seen by then counts as lost (failure)
SYNC_POLL_MS = 30000
CONCIERGE = "@hagency:localhost:8452"
MARK = "perf-probe"

def q(room):
    return urllib.parse.quote(room, safe="")

def account(server, name):
    origin = server[0]
    password = f"Demo-{name}-2026-pass"
    beat7.register(origin, name, password)
    token, mxid = beat7.matrix_login(origin, name, password)
    return {"origin": origin, "token": token, "mxid": mxid, "name": name}

def sync_once(u, since, timeout_ms=SYNC_POLL_MS):
    url = f"{u['origin']}/_matrix/client/v3/sync?timeout={timeout_ms}" + (f"&since={since}" if since else "")
    status, body = call("GET", url, token=u["token"])
    if status != 200:
        raise RuntimeError(f"sync {u['name']}: {status} {body}")
    return body

def wait_membership(u, room, who, membership, timeout=90):
    def seen(s):
        rooms = s.get("rooms", {})
        if membership == "invite":
            return room in rooms.get("invite", {})
        r = rooms.get("join", {}).get(room, {})
        return any(ev.get("state_key") == who and ev.get("content", {}).get("membership") == membership
                   for part in ("state", "timeline") for ev in r.get(part, {}).get("events", []))
    found, since = wait_for(u["origin"], u["token"], seen, timeout)
    if not found:
        sys.exit(f"FAIL: {who} not {membership} in {room} as seen by {u['name']}")
    return since

def shared_room(a, b):
    status, body = call("POST", f"{a['origin']}/_matrix/client/v3/createRoom",
                        {"name": "perf", "preset": "private_chat", "invite": [b["mxid"]]}, a["token"])
    room = body.get("room_id") or sys.exit(f"createRoom: {status} {body}")
    t0 = time.time()
    wait_membership(b, room, b["mxid"], "invite")
    t_invite = time.time() - t0
    t1 = time.time()
    status, body = call("POST", f"{b['origin']}/_matrix/client/v3/join/{q(room)}", {}, b["token"])
    if status != 200:
        sys.exit(f"join: {status} {body}")
    t_join_call = time.time() - t1
    wait_membership(a, room, b["mxid"], "join")
    return room, {"invite_seen": t_invite, "join_call": t_join_call, "join_seen_by_creator": time.time() - t1}

class Receiver(threading.Thread):
    """Long-polls /sync for `room` and stamps the first arrival of each probe."""
    def __init__(self, u, room, since):
        super().__init__(daemon=True)
        self.u, self.room, self.since, self.arrivals, self.stop, self.error = u, room, since, {}, False, None
    def run(self):
        try:
            while not self.stop:
                s = sync_once(self.u, self.since, 5000)
                now = time.time()
                self.since = s.get("next_batch", self.since)
                for ev in s.get("rooms", {}).get("join", {}).get(self.room, {}).get("timeline", {}).get("events", []):
                    body = ev.get("content", {}).get("body", "")
                    if ev.get("type") == "m.room.message" and body.startswith(MARK):
                        self.arrivals.setdefault(body.split()[1], now)
        except Exception as e:  # surfaced by the caller as a FAIL, not swallowed
            self.error = e

def fresh_since(u):
    return sync_once(u, None, 0).get("next_batch")

def timed_messages(sender, receiver, room, n):
    rx = Receiver(receiver, room, fresh_since(receiver)); rx.start()
    time.sleep(0.3)
    sent, put_ms = {}, []
    for i in range(n):
        key = f"{time.time_ns()}-{i}"
        t = time.time()
        status, body = call("PUT", f"{sender['origin']}/_matrix/client/v3/rooms/{q(room)}/send/m.room.message/p{key}",
                            {"msgtype": "m.text", "body": f"{MARK} {key}"}, sender["token"])
        put_ms.append((time.time() - t) * 1000)
        if status != 200:
            sys.exit(f"send: {status} {body}")
        sent[key] = t
        time.sleep(SEND_GAP_S)
    deadline = time.time() + ARRIVAL_TIMEOUT_S
    while len(rx.arrivals) < n and time.time() < deadline and not rx.error:
        time.sleep(0.1)
    rx.stop = True
    if rx.error:
        sys.exit(f"FAIL: receiver sync error: {rx.error}")
    lat = [(rx.arrivals[k] - sent[k]) * 1000 for k in sent if k in rx.arrivals]
    return lat, n - len(lat), put_ms

EVENT_POLL_S = 0.02  # receiver-side poll of GET /event/{id}: isolates federation from the /sync wake tick

def propagation(sender, receiver, room, n):
    """Send -> event readable on the receiver's server, independent of /sync."""
    out, lost = [], 0
    for i in range(n):
        t = time.time()
        status, body = call("PUT", f"{sender['origin']}/_matrix/client/v3/rooms/{q(room)}/send/m.room.message/e{time.time_ns()}",
                            {"msgtype": "m.text", "body": f"{MARK}-direct {i}"}, sender["token"])
        eid = body.get("event_id") or sys.exit(f"send: {status} {body}")
        url = f"{receiver['origin']}/_matrix/client/v3/rooms/{q(room)}/event/{urllib.parse.quote(eid, safe='')}"
        while time.time() - t < ARRIVAL_TIMEOUT_S:
            if call("GET", url, token=receiver["token"])[0] == 200:
                out.append((time.time() - t) * 1000); break
            time.sleep(EVENT_POLL_S)
        else:
            lost += 1
        time.sleep(SEND_GAP_S)
    return out, lost

def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, max(0, int(round(p / 100 * len(xs) + 0.5)) - 1))]

def row(label, xs, lost=0, unit="ms"):
    if not xs:
        return f"{label:<44} {'-':>8} {'-':>8} {'-':>8} {'-':>8}  lost={lost} FAIL"
    return f"{label:<44} {statistics.median(xs):8.1f} {pct(xs,95):8.1f} {max(xs):8.1f} {min(xs):8.1f}  n={len(xs)} lost={lost} {unit}"

def get_version(conn, path="/_matrix/federation/v1/version"):
    conn.request("GET", path)
    r = conn.getresponse(); r.read()
    if r.status != 200:
        raise RuntimeError(f"version: {r.status}")

def fed_timings(n):
    ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
    makers = {
        "direct http 127.0.0.1:8092": lambda: http.client.HTTPConnection("127.0.0.1", 8092, timeout=10),
        "relay https localhost:8452": lambda: http.client.HTTPSConnection("localhost", 8452, timeout=10, context=ctx),
        "relay https 127.0.0.1:8452": lambda: http.client.HTTPSConnection("127.0.0.1", 8452, timeout=10, context=ctx),
    }
    out = []
    for label, mk in makers.items():
        fresh = []
        for _ in range(n):
            t = time.perf_counter(); c = mk(); get_version(c); c.close(); fresh.append((time.perf_counter() - t) * 1000)
        c = mk(); get_version(c); ka = []
        for _ in range(n):
            t = time.perf_counter(); get_version(c); ka.append((time.perf_counter() - t) * 1000)
        c.close()
        out += [row(f"fed /version {label} new-conn", fresh), row(f"fed /version {label} keep-alive", ka)]
    return out

def concierge_run(name):
    u = account(BETA, name)
    status, body = call("POST", f"{u['origin']}/_matrix/client/v3/createRoom", {"is_direct": True, "preset": "trusted_private_chat", "invite": [CONCIERGE]}, u["token"])
    dm = body.get("room_id") or sys.exit(f"createRoom: {status} {body}")
    t0 = time.time()
    since = wait_membership(u, dm, CONCIERGE, "join", 60)
    t_join = time.time() - t0
    t1 = time.time()
    call("PUT", f"{u['origin']}/_matrix/client/v3/rooms/{q(dm)}/send/m.room.message/c{time.time_ns()}", {"msgtype": "m.text", "body": "create agent"}, u["token"])
    created, since = wait_for(u["origin"], u["token"], lambda s: [n for n in notices_from(s, dm, CONCIERGE) if "Created" in n or "already have" in n], 240, since)
    if not created or "Created" not in created[-1]:
        return {"user": name, "fail": f"no Created notice: {created}"}
    t_created = time.time() - t1
    invites, _ = wait_for(u["origin"], u["token"], lambda s: list(s.get("rooms", {}).get("invite", {}).keys()), 60)
    if not invites:
        return {"user": name, "fail": "no agent room invite"}
    room = invites[0]
    call("POST", f"{u['origin']}/_matrix/client/v3/join/{q(room)}", {}, u["token"])
    def puppet_in(s):
        r = s.get("rooms", {}).get("join", {}).get(room, {})
        for ev in r.get("state", {}).get("events", []) + r.get("timeline", {}).get("events", []):
            if ev.get("type") == "m.room.member" and ev.get("state_key", "").startswith("@_hagency_agt_") and ev["content"].get("membership") == "join":
                return ev["state_key"]
    puppet, since2 = wait_for(u["origin"], u["token"], puppet_in, 120)
    if not puppet:
        return {"user": name, "fail": "puppet never joined"}
    t2 = time.time()
    call("PUT", f"{u['origin']}/_matrix/client/v3/rooms/{q(room)}/send/m.room.message/q{time.time_ns()}",
         {"msgtype": "m.text", "body": f"{puppet}: Reply with exactly one word: pong", "m.mentions": {"user_ids": [puppet]}}, u["token"])
    placeholder = lambda t: "请求已收到" in t or "request received" in t.lower()
    first_any, ans = None, None
    deadline = time.time() + 300
    while time.time() < deadline and not ans:
        s = sync_once(u, since2, 20000); since2 = s.get("next_batch", since2)
        msgs = notices_from(s, room, puppet)
        if msgs and first_any is None:
            first_any = time.time() - t2
        real = [m for m in msgs if not placeholder(m)]
        if real:
            ans = (time.time() - t2, real[-1][:60])
    if not ans:
        return {"user": name, "fail": "no real Agent answer in 300 s"}
    return {"user": name, "concierge_join": t_join, "created": t_created, "first_puppet_msg": first_any, "answer": ans[0], "text": ans[1]}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", required=True, help="unique suffix: users are zzperf<tag>a (alpha), b, c (beta)")
    ap.add_argument("-n", type=int, default=20)
    ap.add_argument("--fed-n", type=int, default=50)
    ap.add_argument("--concierge-runs", type=int, default=0)
    ap.add_argument("--only", default="msg,fed,concierge")
    a = ap.parse_args()
    parts, table, created_users = a.only.split(","), [], []
    if "msg" in parts:
        ua = account(ALPHA, f"zzperf{a.tag}a"); created_users.append((ua["name"], "alpha"))
        ub = account(BETA, f"zzperf{a.tag}b"); created_users.append((ub["name"], "beta"))
        uc = account(BETA, f"zzperf{a.tag}c"); created_users.append((uc["name"], "beta"))
        fed_room, setup = shared_room(ua, ub)
        table.append(f"setup alpha->beta: invite seen {setup['invite_seen']*1000:.0f} ms, join call {setup['join_call']*1000:.0f} ms, join seen by alpha {setup['join_seen_by_creator']*1000:.0f} ms")
        local_room, lsetup = shared_room(ub, uc)
        table.append(f"setup beta local: invite seen {lsetup['invite_seen']*1000:.0f} ms, join call {lsetup['join_call']*1000:.0f} ms, join seen {lsetup['join_seen_by_creator']*1000:.0f} ms")
        for label, s, r, room in (("alpha->beta (federated)", ua, ub, fed_room), ("beta->alpha (federated)", ub, ua, fed_room),
                                  ("beta->beta (same server)", ub, uc, local_room)):
            lat, lost, put = timed_messages(s, r, room, a.n)
            table += [row(f"msg visible {label}", lat, lost), row(f"  send PUT {label}", put)]
            plat, plost = propagation(s, r, room, a.n)
            table.append(row(f"  event stored on peer {label}", plat, plost))
    if "fed" in parts:
        table += fed_timings(a.fed_n)
    if "concierge" in parts:
        for i in range(a.concierge_runs):
            name = f"zzperf{a.tag}k{i}"; created_users.append((name, "beta"))
            r = concierge_run(name)
            if "fail" in r:
                table.append(f"concierge {name}: FAIL {r['fail']}")
            else:
                fp = f"{r['first_puppet_msg']:.1f}s" if r["first_puppet_msg"] is not None else "-"
                table.append(f"concierge {name}: join {r['concierge_join']:.1f}s, create->Created {r['created']:.1f}s, "
                             f"first puppet msg {fp}, pong answer {r['answer']:.1f}s ({r['text']!r})")
    print(f"\n{'metric':<44} {'p50':>8} {'p95':>8} {'max':>8} {'min':>8}")
    print("\n".join(table))
    print("\nscrub: " + " ; ".join(f"bash scripts/demo-scrub-user.sh {n} {i}" for n, i in created_users))
    if any("FAIL" in t for t in table):
        sys.exit(1)

if __name__ == "__main__":
    main()
