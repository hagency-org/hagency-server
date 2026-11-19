"""Run Cargo's Agent test executables against isolated databases on the CI PostgreSQL service."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import urlsplit
from uuid import uuid4


def test_targets(artifacts):
    targets = {}
    for line in artifacts.read_text(encoding="utf-8-sig").splitlines():
        msg = json.loads(line)
        if msg["reason"] != "compiler-artifact" or not msg["profile"]["test"] or not msg["executable"]:
            continue
        exe = msg["executable"]
        listing = subprocess.check_output([exe, "--list", "--format", "terse"], text=True)
        targets[exe] = sorted(line.removesuffix(": test") for line in listing.splitlines()
                              if line.endswith(": test"))
    if not any(targets.values()):
        raise ValueError("Cargo produced no Agent tests")
    return targets


def run_shard(index, shards, targets):
    env = os.environ.copy()
    base = urlsplit(env["HAGENCY_AGENT_TEST_DATABASE_URL"])
    with ExitStack() as cleanup:
        for role in ("AGENT", "USAGE"):
            name = f"hagency_{role.lower()}_{uuid4().hex}"
            subprocess.run(["createdb", name], check=True)
            cleanup.callback(subprocess.run, ["dropdb", "--force", name], check=True)
            env[f"HAGENCY_{role}_TEST_DATABASE_URL"] = base._replace(path="/" + name).geturl()
        ran = 0
        for exe, names in targets.items():
            assigned = names[index::shards]
            # Bound the command line on Windows as well as Linux.
            for offset in range(0, len(assigned), 100):
                batch = assigned[offset:offset + 100]
                proc = subprocess.run([exe, "--exact", "--include-ignored", "--test-threads=1", *batch],
                                      env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                      text=True, errors="replace")
                print(proc.stdout, flush=True)
                proc.check_returncode()
                if not re.search(rf"^test result: ok\. {len(batch)} passed; 0 failed; 0 ignored;", proc.stdout, re.M):
                    raise RuntimeError(f"shard {index + 1} did not run all assigned tests")
                ran += len(batch)
        print(f"Shard {index + 1}/{shards} passed {ran} tests", flush=True)
        return ran


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifacts", type=Path)
    parser.add_argument("--shards", type=int, choices=(1, 2, 4), default=4)
    args = parser.parse_args()
    targets = test_targets(args.artifacts)
    with ThreadPoolExecutor(max_workers=args.shards) as pool:
        jobs = [pool.submit(run_shard, i, args.shards, targets) for i in range(args.shards)]
        ran = sum(job.result() for job in jobs)
    expected = sum(map(len, targets.values()))
    if ran != expected:
        raise SystemExit(f"Ran {ran} of {expected} tests")
    print(f"All {ran} Agent tests passed across {args.shards} shards")
