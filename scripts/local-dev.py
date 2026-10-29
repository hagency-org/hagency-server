#!/usr/bin/env python3
"""Persistent, isolated, real Server + native Client development environment."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import time
import re
from urllib.request import ProxyHandler, build_opener
import local_build as builds

ROOT = Path(__file__).resolve().parent.parent
CLIENT = ROOT.parent / "hagency-client"
STATE = ROOT / ".run/local-dev"
ORIGIN = "http://127.0.0.1:8088"
TOOLCHAIN = "+1.99.0"
HTTP = build_opener(ProxyHandler({}))
CLIENT_STATE = Path.home() / "Library/Application Support/org.hagency.local-development"


def run(args, cwd=ROOT, env=None, log=None):
    if log:
        print(f"Running {' '.join(map(str, args[:4]))} … log: {log}", flush=True)
        with open(log, "a") as output:
            result = subprocess.run(args, cwd=cwd, env=env, stdout=output, stderr=output)
        if result.returncode:
            raise RuntimeError(f"Command failed ({result.returncode}); see {log}")
    else:
        subprocess.run(args, cwd=cwd, env=env, check=True)


def private(path, text):
    with open(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        output.write(text)


def compose(*args):
    return ["docker", "compose", "-p", "hagency-full-dev", "--env-file",
            str(STATE / "compose.env"), "-f", str(ROOT / "compose.yaml"), *args]


def identity(pid):
    result = subprocess.run(["ps", "-p", str(pid), "-o", "lstart=,args="],
                            text=True, capture_output=True)
    return result.stdout.strip() if result.returncode == 0 else None


def process(name):
    path = STATE / f"{name}.json"
    if path.exists():
        info = json.loads(path.read_text())
        if identity(info["pid"]) == info["identity"]:
            return info
    return None


def spawn(name, args, cwd=ROOT, env=None, bundle=None, runtime=None):
    with open(STATE / f"{name}.log", "a") as output:
        child = subprocess.Popen(args, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                 stdout=output, stderr=output, start_new_session=True)
    # A shell runner can exec before its stable command identity is recorded.
    time.sleep(1)
    current = identity(child.pid)
    if child.poll() is not None or current is None:
        raise RuntimeError(f"{name} exited; see {STATE / (name + '.log')}")
    builds.atomic_json(STATE / f"{name}.json", {"pid": child.pid, "identity": current,
        "bundle": str(bundle) if bundle else None, "runtime": str(runtime) if runtime else None})


def stop_process(name):
    info = process(name)
    if not info:
        return
    # Only signal a process group created here, with the recorded PID/start time.
    if os.getpgid(info["pid"]) != info["pid"]:
        raise RuntimeError(f"{name} process group changed; refusing to signal it")
    os.killpg(info["pid"], signal.SIGTERM)
    # The server drains claimed work before stopping HTTP and Pasion workers.
    deadline = time.monotonic() + 75
    while process(name) and time.monotonic() < deadline:
        time.sleep(0.2)
    if process(name):
        raise RuntimeError(f"{name} did not stop; inspect its log before retrying")
    (STATE / f"{name}.json").unlink(missing_ok=True)


def get(path):
    with HTTP.open(ORIGIN + path, timeout=3) as response:
        return response.read()


def ready():
    try:
        get("/readyz")
        discovery = json.loads(get("/hagency/api/v1/discovery"))
        return discovery["product"] == "hagency-server" and "project-board-v5" in discovery["capabilities"]
    except (OSError, ValueError, KeyError):
        return False


def wait_ready():
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        if ready():
            return
        if not process("server"):
            raise RuntimeError(f"Server exited; see {STATE / 'server.log'}")
        time.sleep(1)
    raise RuntimeError(f"Server not ready after 180s; see {STATE / 'server.log'}")


def initialize():
    if (STATE / "compose.env").exists():
        for name in ("hagency.toml", "palpo.toml", "pasion.toml", "admin-password"):
            if not (STATE / name).is_file():
                raise RuntimeError(f"Incomplete environment: missing {name}; restore it, do not reinitialize")
        return
    # Never silently attach unknown data or reuse credentials from another deployment.
    volumes = subprocess.check_output(["docker", "volume", "ls", "--format", "{{.Name}}"], text=True)
    if "hagency-full-dev_database" in volumes.splitlines():
        raise RuntimeError("Existing hagency-full-dev_database has no private config; restore .run/local-dev")
    password = secrets.token_hex(24)
    for component in ("hagency", "palpo", "pasion"):
        text = (ROOT / f"config/examples/{component}.toml").read_text()
        text = text.replace("hagency:hagency_dev@", f"hagency:{password}@")
        text = text.replace("../../data", str(STATE / "data"))
        text = text.replace("../../resources", str(ROOT / "resources"))
        if component == "pasion":
            # Disposable local accounts are useful for multi-user acceptance.
            # Keep deployment templates closed and preserve existing profiles.
            text = text.replace("# [account]", "[account]")
            text = text.replace("# password_registration_enabled = true", "password_registration_enabled = true")
            text = text.replace("# password_registration_contact_required = false", "password_registration_contact_required = false")
        private(STATE / f"{component}.toml", text)
    private(STATE / "admin-password", secrets.token_urlsafe(24) + "\n")
    # Written last: a partial setup is reported, never overwritten.
    private(STATE / "compose.env", f"HAGENCY_DB_PASSWORD={password}\n")


def admin_exists():
    """Recover first-run interruption after the durable bootstrap transaction."""
    container = subprocess.check_output(compose("ps", "-q", "postgres"), text=True).strip()
    def sql(statement):
        return subprocess.check_output(["docker", "exec", container, "psql", "-U", "hagency",
            "-d", "pasion", "-v", "ON_ERROR_STOP=1", "-Atc", statement], text=True).strip()
    if sql("select to_regclass('public.users') is not null") == "f":
        return False
    exists = sql("select exists(select 1 from users where username = 'admin' and can_request_admin)")
    return exists == "t"


def configure_bundle(folder):
    for name, field, value in (("hagency", "public_dir", folder / "frontend/public"),
                               ("pasion", "resources_dir", folder / "pasion")):
        path = STATE / f"{name}.toml"
        text = path.read_text()
        text, count = re.subn(rf'(?m)^{field}\s*=.*$', field + " = " + json.dumps(str(value)), text)
        if count != 1:
            raise RuntimeError(f"Expected exactly one {field} in {path}")
        if text != path.read_text():
            backup = path.with_suffix(".toml.before-coherent-build")
            if not backup.exists():
                private(backup, path.read_text())
            temporary = path.with_suffix(".toml.tmp")
            with open(temporary, "w") as output:
                os.chmod(temporary, 0o600)
                output.write(text)
            os.replace(temporary, path)


def start(server_only=False, rebuild_assets=False, restart=False):
    for executable in ("docker", "cargo", "rustup"):
        if not shutil.which(executable):
            raise RuntimeError(f"Missing dependency: {executable}")
    if not server_only and (sys.platform != "darwin" or not (CLIENT / "Cargo.toml").is_file()):
        raise RuntimeError("Native launcher requires macOS and sibling hagency-client; use --server-only otherwise")
    run(["docker", "info"], log=STATE / "build.log")
    if not process("server"):
        with socket.socket() as sock:
            if sock.connect_ex(("127.0.0.1", 8088)) == 0:
                raise RuntimeError("Port 8088 belongs to another process; stop that service first")
    initialize()
    run(compose("up", "-d", "--wait", "postgres"), log=STATE / "build.log")
    # Build and verify everything before replacing running services. Failed builds
    # never publish partial bundles or silently fall back to a different version.
    server_bundle = builds.build(ROOT, STATE, "server", run, force=rebuild_assets)
    client_bundle = None if server_only else builds.build(CLIENT, STATE, "client", run, force=rebuild_assets)
    # A second repository may change while its counterpart is compiling.
    builds.verify(server_bundle, builds.source_state(ROOT, "server"))
    if client_bundle:
        builds.verify(client_bundle, builds.source_state(CLIENT, "client"))
    owner = process("server")
    if restart or not owner or owner.get("bundle") != str(server_bundle):
        stop_process("client")
        stop_process("server")
        configure_bundle(server_bundle)
        command = [str(server_bundle / "hagency-server"), "--config", str(STATE / "hagency.toml")]
        if not admin_exists():
            command += ["--bootstrap-admin", "admin", "--bootstrap-password-file", str(STATE / "admin-password")]
        spawn("server", command, bundle=server_bundle)
    wait_ready()
    receipt = builds.probe(server_bundle, get)
    builds.atomic_json(STATE / "resource-verification.json", {"bundle": str(server_bundle), **receipt})
    owner = process("client")
    if client_bundle and (not owner or owner.get("bundle") != str(client_bundle)):
        stop_process("client")
        env = os.environ.copy()
        # These builds and the runner use the paths above, regardless of caller overrides.
        env.pop("CARGO_TARGET_DIR", None)
        CLIENT_STATE.mkdir(parents=True, exist_ok=True, mode=0o700)
        env["HAGENCY_APP_DATA_DIR"] = str(CLIENT_STATE / "data")
        # LaunchServices packaging is mutable; keep it outside the sealed build.
        # Native resources also stay on the system volume, accessible to the app.
        runtime = CLIENT_STATE / "runtime" / client_bundle.name
        runtime.mkdir(parents=True, exist_ok=True, mode=0o700)
        shutil.copy2(client_bundle / "hagency", runtime / "hagency")
        shutil.copytree(client_bundle / "resources", runtime / "resources", dirs_exist_ok=True)
        manifest = json.loads((client_bundle / "build-manifest.json").read_text())
        verify_runtime(runtime, manifest)
        env["HAGENCY_APP_RESOURCE_DIR"] = str(runtime / "resources")
        spawn("client", ["bash", str(CLIENT / "tools/run-native.sh"), str(runtime / "hagency")], CLIENT, env, client_bundle, runtime)
    status()
    print(f"Login server: {ORIGIN}\nBrowser: {ORIGIN}/login\nAccount: admin\nPassword file: {STATE / 'admin-password'}")


def status():
    for name in ("server", "client"):
        info = process(name)
        print(f"{name}: " + (f"running (PID {info['pid']})" if info else "stopped"))
    print(f"Backend ready (including project-board-v5): {ready()}\nLogs and private config: {STATE}")


def verify_runtime(runtime, manifest):
    for name, record in manifest["artifacts"].items():
        path = runtime / name
        if not path.is_file() or builds.sha(path) != record["sha256"]:
            raise RuntimeError(f"Native runtime copy differs from verified build: {name}")


def update_main():
    for root in (ROOT, CLIENT):
        branch = subprocess.check_output(["git", "branch", "--show-current"], cwd=root, text=True).strip()
        if branch != "main":
            raise RuntimeError(f"{root.name} is on {branch}; switch to main before update")
    for root in (ROOT, CLIENT):
        run(["git", "fetch", "origin", "main"], cwd=root)
        # Git preserves local changes and refuses divergence/overwrites. No
        # automatic stash, reset, rebase or force update is performed.
        run(["git", "merge", "--ff-only", "origin/main"], cwd=root)


def doctor(server_only=False):
    for name, root in (("server", ROOT), ("client", CLIENT)):
        if name == "client" and server_only:
            continue
        owner = process(name)
        if not owner or not owner.get("bundle"):
            raise RuntimeError(f"{name} is not running from a verified build bundle")
        folder = Path(owner["bundle"])
        manifest = builds.verify(folder, builds.source_state(root, name))
        if name == "server":
            if not ready():
                raise RuntimeError("Backend startup roundtrip is not ready")
            print(json.dumps(builds.probe(folder, get)))
        elif owner.get("runtime"):
            verify_runtime(Path(owner["runtime"]), manifest)
        print(f"PASS {name}: source provenance, complete artifacts and checksums")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=("start", "stop", "restart", "status", "doctor", "update", "logs"), default="start")
    parser.add_argument("--server-only", action="store_true")
    parser.add_argument("--rebuild-assets", action="store_true", help="Force complete source builds, including both browser WASM apps")
    parser.add_argument("--pull", action="store_true", help="Fast-forward both main branches from origin before building")
    args = parser.parse_args()
    os.umask(0o077)
    STATE.mkdir(parents=True, exist_ok=True, mode=0o700)
    # Avoid caller-specific output locations and toolchain selection.
    os.environ.pop("CARGO_TARGET_DIR", None)
    os.environ.pop("RUSTUP_TOOLCHAIN", None)
    if args.command == "status":
        status()
        return
    if args.command == "doctor":
        doctor(args.server_only)
        return
    if args.command == "logs":
        for name in ("server", "client", "build"):
            path = STATE / f"{name}.log"
            print(f"\n{path}")
            if path.exists():
                print("\n".join(path.read_text(errors="replace").splitlines()[-30:]))
        return
    with open(STATE / "launcher.lock", "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError("Another start/stop is in progress; use status or logs") from None
        if args.command == "update" or args.pull:
            update_main()
        if args.command == "stop":
            stop_process("client")
            stop_process("server")
            if (STATE / "compose.env").exists():
                run(compose("stop", "postgres"), log=STATE / "build.log")
        if args.command in ("start", "restart"):
            start(args.server_only, args.rebuild_assets, restart=args.command == "restart")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"local-dev: {error}", file=sys.stderr)
        sys.exit(1)
