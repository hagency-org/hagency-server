#!/usr/bin/env python3
"""Full from-zero demo rehearsal through the DEMO-ONLY automation hook.
usage: rehearsal.py <suffix>   (e.g. r1) — creates users ann-<s>, ben-<s>."""
import json, subprocess, sys, time, urllib.request, urllib.error

S = sys.argv[1]
ROOT = "/Users/lirongxie/claudeworkfolder/hagency"
PW = "Demo-" + S + "-rehearsal-pass"   # throwaway local test password
REPO = "csheargm/hagency-demo-sandbox"
results = []

def hook(who, method, path, body=None, timeout=600):
    f = json.load(open(f"{ROOT}/hagency-desktop/.run/demo-{who}/demo-automation.json"))
    req = urllib.request.Request(f"http://127.0.0.1:{f['port']}{path}", method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Authorization": f"Bearer {f['token']}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"{}")

def gh(*a):
    return subprocess.run(["gh", *a], capture_output=True, text=True).stdout.strip()

def record(scene, t0, approvals=0, evidence=""):
    dt = round(time.time() - t0)
    results.append((scene, dt, approvals, evidence))
    print(f"[{S}] {scene}: {dt}s, approvals {approvals} {evidence}", flush=True)

def wait(pred, secs, step=2):
    end = time.time() + secs
    while time.time() < end:
        v = pred()
        if v: return v
        time.sleep(step)
    return None

def drive(who, done, secs):
    """Approve once whenever an approval is pending, until done() is truthy."""
    n, end = 0, time.time() + secs
    while time.time() < end:
        if hook(who, "GET", "/state").get("pending_approvals", 0):
            hook(who, "POST", "/approval", {"decision": "once"}); n += 1
        v = done()
        if v: return v, n
        time.sleep(3)
    return None, n

def members(room, who="ann"):
    return hook(who, "GET", f"/room/members?room_id={room}")["members"]

def joined(room, name):
    return any(m["name"] == name and m["membership"] == "join" for m in members(room))

# Scene 1: sign up, create agents, project room (maintainer auto-joins)
t = time.time()
for who in ("ann", "ben"):
    if not hook(who, "GET", "/state").get("user_id"):
        try:
            print(who, hook(who, "POST", "/signup", {"username": f"{who}-{S}", "password": PW}), flush=True)
        except urllib.error.HTTPError as e:
            sys.exit(f"{who} signup failed: {e.code} {e.read().decode()}")
record("S1a both sign up", t)
t = time.time()
hook("ann", "POST", "/agent/create", {"name": "maintainer", "harness": "claude"})
hook("ben", "POST", "/agent/create", {"name": "contributor", "harness": "codex"})
record("S1b both create agents", t)
t = time.time()
room = hook("ann", "POST", "/room/create", {"name": "hagency-desktop", "topic": "OSS project room"})["room_id"]
ok = wait(lambda: joined(room, "maintainer"), 60, 1)
record("S1c project room + maintainer auto-joins", t, evidence="" if ok else "FAIL")

# Scene 2: contributor from the other server
t = time.time()
hook("ann", "POST", "/room/invite", {"room_id": room, "user_id": hook("ben", "GET", "/state")["user_id"]})
wait(lambda: any(i["room_id"] == room for i in hook("ben", "GET", "/state").get("invites") or []), 30, 1)
hook("ben", "POST", "/room/join", {"room_id": room})
hook("ben", "POST", "/agent/invite", {"room_id": room, "agent": "contributor"})
ok = wait(lambda: joined(room, "contributor"), 60, 1)
record("S2 contributor + agent join across servers", t, evidence="" if ok else "FAIL")

# Scene 3: triage from the room discussion -> issue
hook("ben", "POST", "/message", {"room_id": room, "text": "Docs gap: README.md on main doesn't say how to run the desktop app locally."})
hook("ann", "POST", "/message", {"room_id": room, "text": "Agreed, newcomers keep asking about that."})
before = gh("issue", "list", "--repo", REPO, "--state", "all", "--limit", "1", "--json", "number", "-q", ".[0].number")
t = time.time()
hook("ann", "POST", "/message", {"room_id": room, "mention": "maintainer",
     "text": f"triage the report above against {REPO} main, then file an issue and post the URL here."})
def triaged():
    import re
    latest = gh("issue", "list", "--repo", REPO, "--state", "all", "--limit", "1", "--json", "number", "-q", ".[0].number")
    if latest != before:
        return latest
    # A correct triage may point at an existing issue instead of filing a duplicate.
    replies = [e.get("body") or "" for e in hook("ann", "GET", f"/room/messages?room_id={room}&limit=40")["events"]
               if "_hagency_agt_" in e.get("sender", "") and "Working" not in (e.get("body") or "")]
    found = re.findall(rf"github.com/{REPO}/issues/(\d+)", " ".join(replies))
    return found[-1] if found else None
new, n = drive("ann", triaged, 400)
record("S3 triage -> issue", t, n, f"#{new}" if new else "FAIL")

# Scene 4: contributor's Codex agent opens a PR
branch = f"agent/contributor-{S}"
t = time.time()
hook("ben", "POST", "/message", {"room_id": room, "mention": "contributor",
     "text": f"open a small docs pull request on {REPO} fixing issue #{new}: add a short 'Run locally' section to README.md; branch {branch}; Co-authored-by trailer for yourself; post the PR URL here."})
pr, n = drive("ben", lambda: gh("pr", "list", "--repo", REPO, "--head", branch, "--json", "number", "-q", ".[0].number") or None, 600)
record("S4 contributor PR", t, n, f"PR #{pr}" if pr else "FAIL")

# Scene 5: maintainer review
if pr:
    rv0 = int(gh("api", f"repos/{REPO}/pulls/{pr}/reviews", "--jq", "length") or 0)
    t = time.time()
    hook("ann", "POST", "/message", {"room_id": room, "mention": "maintainer",
         "text": f"review pull request #{pr} on {REPO}, leave one review comment, and post a one-line summary here."})
    ok, n = drive("ann", lambda: int(gh("api", f"repos/{REPO}/pulls/{pr}/reviews", "--jq", "length") or 0) > rv0, 400)
    record("S5 maintainer review", t, n, "" if ok else "FAIL")

# Live progress evidence: a status message that ended in "Done in"
evs = hook("ann", "GET", f"/room/messages?room_id={room}&limit=80")["events"]
record("live progress statuses with 'Done in'", time.time(), evidence=str(sum("Done in" in (e.get("body") or "") for e in evs)))

# Scene 6: newcomer in the browser builds a page (headless Element-equivalent)
t = time.time()
out = subprocess.run(["python3", f"{ROOT}/hagency-server/scripts/demo-build-app-e2e.py", "--user", f"new-{S}"],
                     capture_output=True, text=True).stdout
record("S6 newcomer builds a page", t, evidence="PASS" if "PASS" in out else "FAIL")

json.dump({"suffix": S, "room": room, "results": results}, open(f"{ROOT}/evaluation/rehearsal-{S}.json", "w"), indent=1)
print(json.dumps(results, indent=1))
