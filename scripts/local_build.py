"""Source-bound local build bundles. No image imports or loose resource reuse."""
import hashlib
from html.parser import HTMLParser
import json
import os
import re
from pathlib import Path
import posixpath
import shutil
import subprocess
import tempfile
import uuid

SCHEMA = 1
TOOLCHAIN = "+1.99.0"


def sha(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def atomic_json(path, value):
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as output:
        os.chmod(output.name, 0o600)
        json.dump(value, output, ensure_ascii=False, indent=2)
        name = output.name
    os.replace(name, path)


def source_state(root, kind):
    roots = ["Cargo.toml", "Cargo.lock", "crates", ".cargo/config.toml"]
    roots += ["xtask", "scripts/local-dev.py", "scripts/local_build.py"] if kind == "server" else ["tools", ".octos-workspace.toml"]
    files = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *roots], cwd=root)
    names = set(files.decode().split("\0")) - {""}
    # The local Cargo configuration may intentionally be ignored by Git.
    if (root / ".cargo/config.toml").is_file():
        names.add(".cargo/config.toml")
    digest = hashlib.sha256()
    for name in sorted(names):
        path = root / name
        digest.update(name.encode() + b"\0")
        digest.update((sha(path) if path.is_file() else "deleted").encode())
    flags = {key: os.environ.get(key, "") for key in
             ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")}
    digest.update(json.dumps(flags, sort_keys=True).encode())
    return {"root": str(root.resolve()), "kind": kind, "toolchain": TOOLCHAIN,
            "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
            "branch": subprocess.check_output(["git", "branch", "--show-current"], cwd=root, text=True).strip(),
            "inputs": digest.hexdigest()}


def inventory(folder):
    result = {}
    for path in sorted(folder.rglob("*")):
        if path.is_file() and path.name != "build-manifest.json":
            if path.is_symlink():
                raise RuntimeError(f"Build artifacts must be regular files: {path}")
            result[str(path.relative_to(folder))] = {"sha256": sha(path), "bytes": path.stat().st_size}
    return result


def validate_layout(folder, kind):
    if kind == "client":
        required = ["hagency", "resources/hagency_app/resources/icon_32.png"]
    else:
        required = ["hagency-server", "frontend/public/index.html", "pasion/public/index.html",
                    "pasion/templates/app.html", *["frontend/public/assets/" + name for name in
                    ("hagency-mark.svg", "hagency-theme.css", "hagency-theme.js")]]
    for name in required:
        if not (folder / name).is_file() or (folder / name).stat().st_size == 0:
            raise RuntimeError(f"Incomplete {kind} build: missing {name}")
    if kind == "server":
        for name in ("frontend/public/index.html", "pasion/templates/app.html"):
            html = (folder / name).read_text()
            if '/assets/hagency-theme.css' not in html or '/assets/hagency-mark.svg' not in html:
                raise RuntimeError(f"{name} does not load the shared main-branch branding")
        for name in ("frontend/public", "pasion/public"):
            if not list((folder / name).rglob("*.wasm")):
                raise RuntimeError(f"Missing compiled WASM: {name}")


def seal(folder, source):
    validate_layout(folder, source["kind"])
    atomic_json(folder / "build-manifest.json", {"schema": SCHEMA, "source": source, "artifacts": inventory(folder)})


def verify(folder, source):
    manifest = json.loads((folder / "build-manifest.json").read_text())
    if manifest.get("schema") != SCHEMA or manifest.get("source") != source:
        raise RuntimeError("Build provenance does not match the current source/branch/toolchain")
    validate_layout(folder, source["kind"])
    if manifest["artifacts"] != inventory(folder):
        raise RuntimeError("Build artifact checksum mismatch; loose/copied/tampered files are refused")
    return manifest


def build(root, state, kind, run, force=False):
    source = source_state(root, kind)
    print(f"{kind} source: {source['branch']} {source['head'][:12]}, inputs {source['inputs'][:12]}", flush=True)
    pointer = state / f"{kind}-build.json"
    if pointer.exists() and not force:
        try:
            folder = Path(json.loads(pointer.read_text())["path"])
            if folder.parent.resolve() != (state / "builds").resolve():
                raise RuntimeError("Build pointer is outside the managed build directory")
            verify(folder, source)
            print(f"Verified {kind} build: {folder.name}", flush=True)
            return folder
        except (OSError, ValueError, KeyError, RuntimeError) as error:
            print(f"Rebuilding complete {kind} bundle: {error}", flush=True)
    folder = state / "builds" / f"{kind}-{source['head'][:8]}-{source['inputs'][:10]}-{uuid.uuid4().hex[:8]}"
    folder.mkdir(parents=True, mode=0o700)
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(root / "target")
    env["RUSTUP_TOOLCHAIN"] = "1.99.0"
    log = state / "build.log"
    if kind == "server":
        run(["rustup", "target", "add", "--toolchain", "1.99.0", "wasm32-unknown-unknown"], env=env, log=log)
        # Both complete browser apps are produced from this checkout and its lock.
        for task, output in (("prepare-frontend", folder / "frontend/public"), ("prepare-pasion", folder / "pasion")):
            run(["cargo", TOOLCHAIN, "run", "--locked", "--manifest-path", "xtask/Cargo.toml", "--",
                 task, "--output", str(output)], env=env, log=log)
        run(["cargo", TOOLCHAIN, "build", "--locked", "-p", "hagency-server", "--bin", "hagency-server"], env=env, log=log)
        shutil.copy2(root / "target/debug/hagency-server", folder / "hagency-server")
    else:
        run(["cargo", TOOLCHAIN, "build", "--locked", "-p", "hagency-app", "--bin", "hagency"], cwd=root, env=env, log=log)
        shutil.copy2(root / "target/debug/hagency", folder / "hagency")
        with open(log, "a") as errors:
            metadata = json.loads(subprocess.check_output(["cargo", TOOLCHAIN, "metadata", "--locked", "--format-version", "1"],
                                                         cwd=root, env=env, text=True, stderr=errors))
        for package in metadata["packages"]:
            resources = Path(package["manifest_path"]).parent / "resources"
            if resources.is_dir():
                shutil.copytree(resources, folder / "resources" / package["name"].replace("-", "_") / "resources", dirs_exist_ok=True)
    if source_state(root, kind) != source:
        raise RuntimeError("Source changed during the build; this bundle was not published. Retry start.")
    seal(folder, source)
    atomic_json(pointer, {"path": str(folder)})
    return folder


class References(HTMLParser):
    def __init__(self):
        super().__init__()
        self.urls = []

    def add(self, url):
        if url.startswith("/"):
            # Dioxus uses /./assets/ in generated preload and module URLs;
            # browsers normalize that path before requesting the resource.
            path = posixpath.normpath(url.split("?", 1)[0].split("#", 1)[0])
            if path.startswith(("/assets/", "/auth/assets/")):
                self.urls.append(path)

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        key = "src" if tag in ("script", "img") else "href" if tag == "link" else None
        if key:
            self.add(attributes.get(key, ""))

    def handle_data(self, data):
        # Dioxus emits inline ES-module imports instead of a script src attribute.
        for url in re.findall(r'''["']((?:/auth)?/(?:\./)*assets/[A-Za-z0-9_./-]+)["']''', data):
            self.add(url)


def probe(folder, get):
    """Verify bytes actually served, including both entrypoints and their assets."""
    urls = set()
    for page in ("/login", "/auth/login", "/auth/register"):
        body = get(page)
        if page == "/login" and body != (folder / "frontend/public/index.html").read_bytes():
            raise RuntimeError("Served entrypoint differs from the verified frontend build")
        html = body.decode()
        if '/assets/hagency-theme.css' not in html:
            raise RuntimeError(f"Served {page} is missing the shared theme")
        parser = References()
        parser.feed(html)
        urls.update(parser.urls)
    for url in urls:
        relative = "pasion/public" + url[len("/auth"):] if url.startswith("/auth/assets/") else "frontend/public" + url
        expected = folder / relative
        if not expected.is_file() or hashlib.sha256(get(url)).hexdigest() != sha(expected):
            raise RuntimeError(f"Served asset differs from the verified build: {url}")
    # Validate compiled payloads as well as the visible CSS/JS/logo.
    for directory, prefix in (("frontend/public", ""), ("pasion/public", "/auth")):
        for path in (folder / directory).rglob("*.wasm"):
            url = prefix + "/" + str(path.relative_to(folder / directory))
            if hashlib.sha256(get(url)).hexdigest() != sha(path):
                raise RuntimeError(f"Served WASM differs from the verified build: {url}")
    return {"verifiedReferences": len(urls), "sharedTheme": "/assets/hagency-theme.css"}
