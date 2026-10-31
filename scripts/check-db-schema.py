#!/usr/bin/env python3
"""Generate/check Diesel definitions using the real startup upgrade path in a disposable DB."""
import argparse
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="replace files with Diesel CLI output")
    args = parser.parse_args()
    url = os.environ.get("HAGENCY_SCHEMA_DATABASE_URL")
    if not url:
        parser.error("HAGENCY_SCHEMA_DATABASE_URL must name a disposable schema-generation database")
    env = dict(os.environ, DATABASE_URL=url)
    subprocess.run(["cargo", "run", "--locked", "-p", "hagency-agent-service", "--example", "prepare_schema"],
                   cwd=ROOT, env=env, check=True)
    for key in ("agent", "project"):
        config = "diesel.toml" if key == "agent" else "diesel-project.toml"
        generated = subprocess.check_output(["diesel", "print-schema", "--config-file", config],
                                            cwd=ROOT, env=env, text=True)
        path = ROOT / "crates/agent-service/src/schema" / f"{key}.rs"
        if args.write:
            path.write_text(generated, encoding="utf-8", newline="\n")
        elif path.read_text(encoding="utf-8") != generated:
            raise SystemExit(f"{path.relative_to(ROOT)} differs from Diesel CLI output; run with --write")
    print("Diesel schemas match the complete startup upgrade path")


if __name__ == "__main__":
    main()
