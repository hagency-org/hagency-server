#!/usr/bin/env python3
"""Runs a project's frozen black-box cases (acceptance/fixtures/<project>/cases.json) against a fresh
container of the deployed image. Standard library only; needs Docker.

    blackbox.py <cases.json> <image> --release <tag> --sha <commit> [--port 8080]

Prints one JSON object: {"sections": {"startup"|"http"|"container": {"passed", "failed": [...]}}}.
Exit 0 when every case passed, 1 when any failed, 2 on a usage or setup error.

Case fields (all optional except name): startup cases run the image with `env` and expect
`exit_code`, `stderr_contains`, `stderr_excludes`; variables they omit are passed empty. HTTP cases send `method` `path` with `body` (JSON)
or `raw_body`, and expect `status`, `content_type` (prefix), `json` (subset match), `json_has`
(keys present), `json_array_length`, `json_array_ids` (the `id` of each element, in order);
`save` keeps response fields for later `${name}`. `${release}` and `${sha}` are the Release values.
Container cases: `non_root` (the image does not run as root) and `healthcheck` (the image declares
one).

CLI cases run `command` from the image's PATH with `args`, the project's fixture directory mounted
read-only at /fixtures (`${fixtures}` in args), and `stdin_file` as standard input. Expectations name
fixture files, so the frozen expected outputs stay the single source: `exit_code` (or
`exit_code_from`, a JSON file's `exit_code`), `stdout_json_file` (subset match after dropping
`exit_code`; lists match element by element and must be the same length), `stdout_file` (byte for
byte) and `stdout_trimmed_file` (equal after trimming whitespace). `file#key` picks one member of a
JSON file.
"""
import argparse
import json
import os
import re
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

PREFIX = "gitfab-blackbox"


def substitute(value, names):
    if isinstance(value, str):
        return re.sub(r"\$\{(\w+)\}", lambda m: str(names.get(m.group(1), m.group(0))), value)
    if isinstance(value, list):
        return [substitute(v, names) for v in value]
    if isinstance(value, dict):
        return {k: substitute(v, names) for k, v in value.items()}
    return value


def subset(want, got, at="$"):
    """Differences where `got` does not contain `want`: dicts by key, lists element by element with
    the same length, everything else equal."""
    if isinstance(want, list):
        if not isinstance(got, list):
            return [f"{at}: expected an array"]
        if len(want) != len(got):
            return [f"{at}: {len(got)} elements, expected {len(want)}"]
        out = []
        for i, (w, g) in enumerate(zip(want, got)):
            out.extend(subset(w, g, f"{at}[{i}]"))
        return out
    if isinstance(want, dict):
        if not isinstance(got, dict):
            return [f"{at}: expected an object"]
        out = []
        for k, v in want.items():
            if k not in got:
                out.append(f"{at}.{k}: missing")
            else:
                out.extend(subset(v, got[k], f"{at}.{k}"))
        return out
    return [] if want == got else [f"{at}: expected {json.dumps(want, ensure_ascii=False)}, got {json.dumps(got, ensure_ascii=False)}"]


def docker(*args, timeout=60):
    return subprocess.run(["docker", *args], capture_output=True, text=True, timeout=timeout)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def env_args(env):
    out = []
    for k, v in env.items():
        out += ["-e", f"{k}={v}"]
    return out


def run_startup(cases, image, names, service_env):
    """Each case runs with exactly its own environment: a variable the service or another case sets but
    this case omits is passed empty, since the image itself may carry a value (a Release baked in)."""
    known = set(service_env)
    for case in cases:
        known |= set(case.get("env", {}))
    failed = []
    for case in cases:
        case = substitute(case, names)
        env = {k: "" for k in known}
        env.update(case.get("env", {}))
        name = f"{PREFIX}-startup-{int(time.time() * 1000)}"
        try:
            p = docker("run", "--rm", "--name", name, *env_args(env), image, timeout=30)
        except subprocess.TimeoutExpired:
            docker("rm", "-f", name)
            failed.append({"case": case["name"], "why": "still running after 30 s; expected it to exit"})
            continue
        why = []
        if "exit_code" in case and p.returncode != case["exit_code"]:
            why.append(f"exit {p.returncode}, expected {case['exit_code']}")
        for text in case.get("stderr_contains", []):
            if text not in p.stderr:
                why.append(f"stderr lacks {text!r}")
        for text in case.get("stderr_excludes", []):
            if text in p.stderr or text in p.stdout:
                why.append(f"output contains {text!r}")
        if why:
            failed.append({"case": case["name"], "why": "; ".join(why)})
    return {"passed": len(cases) - len(failed), "failed": failed}


def request(base, case):
    data = None
    headers = {}
    if "raw_body" in case:
        data = case["raw_body"].encode()
        headers["Content-Type"] = "application/json"
    elif "body" in case:
        data = json.dumps(case["body"], ensure_ascii=False).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + case["path"], data=data, method=case.get("method", "GET"), headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, r.headers.get("Content-Type", ""), r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers.get("Content-Type", ""), e.read()


def run_http(cases, base, names):
    failed = []
    for raw in cases:
        case = substitute(raw, names)
        try:
            status, ctype, body = request(base, case)
        except OSError as e:
            failed.append({"case": case["name"], "why": f"request failed: {e}"})
            continue
        why = []
        if "status" in case and status != case["status"]:
            why.append(f"status {status}, expected {case['status']}")
        if "content_type" in case and not ctype.startswith(case["content_type"]):
            why.append(f"content type {ctype!r}")
        parsed = None
        if any(k in case for k in ("json", "json_has", "json_array_length", "json_array_ids", "save")):
            try:
                parsed = json.loads(body or b"null")
            except ValueError:
                why.append("response is not JSON")
        if parsed is not None or "json" in case:
            if "json" in case:
                why += subset(case["json"], parsed)
            for key in case.get("json_has", []):
                if not isinstance(parsed, dict) or key not in parsed:
                    why.append(f"$.{key}: missing")
            if "json_array_length" in case and (not isinstance(parsed, list) or len(parsed) != case["json_array_length"]):
                why.append(f"expected an array of {case['json_array_length']}")
            if "json_array_ids" in case:
                ids = [x.get("id") for x in parsed] if isinstance(parsed, list) else None
                if ids != case["json_array_ids"]:
                    why.append(f"ids {ids}, expected {case['json_array_ids']}")
            for var, field in case.get("save", {}).items():
                if isinstance(parsed, dict) and field in parsed:
                    names[var] = parsed[field]
                else:
                    why.append(f"cannot save {field}")
        if why:
            failed.append({"case": case["name"], "why": "; ".join(why)})
    return {"passed": len(cases) - len(failed), "failed": failed}


def fixture(root, ref):
    """Bytes of a fixture file, or the JSON member `file#key`, under the fixture root only."""
    name, _, key = ref.partition("#")
    path = os.path.realpath(os.path.join(root, name))
    if not path.startswith(os.path.realpath(root) + os.sep):
        raise ValueError(f"{ref}: outside the fixtures")
    data = open(path, "rb").read()
    if key:
        return json.loads(data)[key]
    return data


def run_cli(cases, image, root, names):
    failed = []
    names = dict(names, fixtures="/fixtures")
    for case in cases:
        case = substitute(case, names)
        why = []
        try:
            stdin = fixture(root, case["stdin_file"]) if "stdin_file" in case else None
            name = f"{PREFIX}-cli-{int(time.time() * 1000)}"
            cmd = ["docker", "run", "--rm", "-i", "--name", name, "-v", f"{root}:/fixtures:ro",
                   "--entrypoint", case["command"], image, *case.get("args", [])]
            p = subprocess.run(cmd, input=stdin, capture_output=True, timeout=60)
        except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as e:
            failed.append({"case": case["name"], "why": f"could not run: {e}"})
            continue
        want_code = case.get("exit_code")
        if "exit_code_from" in case:
            source = fixture(root, case["exit_code_from"])
            want_code = (json.loads(source) if isinstance(source, bytes) else source).get("exit_code")
        if want_code is not None and p.returncode != want_code:
            why.append(f"exit {p.returncode}, expected {want_code}")
        try:
            if "stdout_file" in case and p.stdout != fixture(root, case["stdout_file"]):
                why.append(f"stdout differs from {case['stdout_file']}")
            if "stdout_trimmed_file" in case and p.stdout.strip() != fixture(root, case["stdout_trimmed_file"]).strip():
                why.append(f"stdout differs from {case['stdout_trimmed_file']}")
            if "stdout_json_file" in case:
                want = fixture(root, case["stdout_json_file"])
                want = json.loads(want) if isinstance(want, bytes) else want
                if isinstance(want, dict):
                    want = {k: v for k, v in want.items() if k != "exit_code"}
                try:
                    got = json.loads(p.stdout or b"null")
                    why += subset(want, got)
                except ValueError:
                    why.append("stdout is not JSON")
        except (OSError, ValueError, KeyError) as e:
            why.append(f"expected output: {e}")
        if why:
            failed.append({"case": case["name"], "why": "; ".join(why)[:300]})
    return {"passed": len(cases) - len(failed), "failed": failed}


def run_container(spec, image):
    failed = []
    p = docker("image", "inspect", "-f", "{{json .Config}}", image)
    config = json.loads(p.stdout) if p.returncode == 0 else {}
    checks = 0
    if spec.get("non_root"):
        checks += 1
        user = (config.get("User") or "").split(":")[0]
        if user in ("", "root", "0"):
            failed.append({"case": "non_root", "why": f"image runs as {user or 'root (no USER)'}"})
    if spec.get("healthcheck"):
        checks += 1
        if not (config.get("Healthcheck") or {}).get("Test"):
            failed.append({"case": "healthcheck", "why": "image declares no HEALTHCHECK"})
    return {"passed": checks - len(failed), "failed": failed}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cases")
    ap.add_argument("image")
    ap.add_argument("--release", required=True)
    ap.add_argument("--sha", required=True)
    ap.add_argument("--port", type=int, default=8080, help="port the service listens on in the container")
    ap.add_argument("--wait", type=int, default=30)
    a = ap.parse_args()
    try:
        cases = json.load(open(a.cases, encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(json.dumps({"error": f"cases: {e}"}))
        return 2
    names = {"release": a.release, "sha": a.sha}
    sections = {}
    if cases.get("startup"):
        sections["startup"] = run_startup(cases["startup"], a.image, names, cases.get("service", {}).get("env", {}))
    if cases.get("http"):
        service = substitute(cases.get("service", {}), names)
        port = free_port()
        name = f"{PREFIX}-{port}"
        started = docker("run", "-d", "--rm", "--name", name, "-p", f"127.0.0.1:{port}:{a.port}",
                         *env_args(service.get("env", {})), a.image)
        if started.returncode != 0:
            print(json.dumps({"error": f"cannot start {a.image}: {started.stderr.strip()[-200:]}"}))
            return 2
        try:
            base = f"http://127.0.0.1:{port}"
            ready = service.get("ready", "/")
            deadline = time.time() + a.wait
            while True:
                try:
                    if request(base, {"path": ready})[0] == 200:
                        break
                except OSError:
                    pass
                if time.time() > deadline:
                    sections["http"] = {"passed": 0, "failed": [{"case": "ready", "why": f"{ready} not 200 within {a.wait} s"}]}
                    break
                time.sleep(0.5)
            if "http" not in sections:
                sections["http"] = run_http(cases["http"], base, names)
        finally:
            docker("rm", "-f", name)
    if cases.get("cli"):
        sections["cli"] = run_cli(cases["cli"], a.image, os.path.dirname(os.path.abspath(a.cases)), names)
    if cases.get("container"):
        sections["container"] = run_container(cases["container"], a.image)
    ok = all(not s["failed"] for s in sections.values())
    print(json.dumps({"ok": ok, "sections": sections}, ensure_ascii=False))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
