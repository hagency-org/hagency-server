#!/usr/bin/env python3
"""Reject new, changed, expired or stale dependency-risk dispositions.

Reviewed unresolved findings stay visible. A successful policy check is not
a clean cargo-audit result. No RustSec advisory is hidden with --ignore.
"""
import argparse
from datetime import date, datetime, timedelta
import json
from pathlib import Path
import re
import subprocess
import sys
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parents[1]


def review_date(policy, now):
    """Review dates use the declared calendar, independent of runner timezone."""
    return now.astimezone(ZoneInfo(policy["timezone"])).date()


def findings(report):
    vulnerabilities = report["vulnerabilities"]["list"]
    if report["vulnerabilities"]["count"] != len(vulnerabilities):
        raise ValueError("inconsistent audit vulnerability count")
    records = [("vulnerability", entry) for entry in vulnerabilities]
    for kind, entries in report["warnings"].items():
        records.extend((kind, entry) for entry in entries)
    result = set()
    for kind, entry in records:
        key = (entry["advisory"]["id"], kind, entry["package"]["name"], entry["package"]["version"])
        if key in result:
            raise ValueError("duplicate audit finding")
        result.add(key)
    return result


def check(report, policy, lock, today):
    errors = []
    if policy["version"] != 1:
        raise ValueError("unsupported dependency policy")
    reviewed = date.fromisoformat(policy["reviewed_on"])
    expires = date.fromisoformat(policy["expires_on"])
    if not reviewed <= today <= expires or not reviewed <= expires <= reviewed + timedelta(days=30):
        errors.append("dependency risk review is expired, future-dated or longer than 30 days")
    sources = {}
    for block in re.split(r"(?m)^\[\[package\]\]\s*$", lock)[1:]:
        fields = dict(re.findall(r'(?m)^(name|source) = "([^"\n]+)"$', block))
        if "name" in fields:
            sources.setdefault(fields["name"], set()).add(fields.get("source"))
    for package, source in policy["reviewed_sources"].items():
        if sources.get(package) != {source}:
            errors.append("reviewed upstream source changed: " + package)
    accepted = set()
    for row in policy["findings"]:
        key = (row["advisory"], row["kind"], row["package"], row["version"])
        if key in accepted or not row["reason"].strip() or not row["reference"].startswith("https://"):
            raise ValueError("invalid or duplicate dependency disposition")
        if row["kind"] == "vulnerability" and row["status"] != "unresolved_upstream":
            raise ValueError("a vulnerable version cannot be described as resolved")
        accepted.add(key)
    actual = findings(report)
    errors.extend("unreviewed finding: " + " / ".join(row) for row in sorted(actual - accepted))
    errors.extend("remove stale disposition: " + " / ".join(row) for row in sorted(accepted - actual))
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-fetch", action="store_true", help="use an already fetched advisory database")
    parser.add_argument("--report", type=Path, default=ROOT / ".run/dependency-audit.json")
    args = parser.parse_args()
    command = ["cargo", "audit", "--file", str(ROOT / "Cargo.lock"), "--json"]
    if args.no_fetch:
        command.append("--no-fetch")
    result = subprocess.run(command, cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode not in (0, 1):
        raise ValueError("cargo-audit could not complete: " + result.stderr.strip())
    report = json.loads(result.stdout)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    policy = json.loads((ROOT / "security/dependency-policy.json").read_text())
    errors = check(report, policy, (ROOT / "Cargo.lock").read_text(), review_date(policy, datetime.now().astimezone()))
    print("Audit: {} unresolved vulnerability records; {} informational warnings.".format(
        report["vulnerabilities"]["count"], sum(len(rows) for rows in report["warnings"].values())))
    for row in policy["findings"]:
        print("{} {} {}: {}".format(row["status"], row["advisory"], row["package"], row["reason"]))
    for error in errors:
        print("ERROR: " + error, file=sys.stderr)
    if not errors:
        print("Dependency policy satisfied until {}; unresolved risks remain recorded.".format(policy["expires_on"]))
    return bool(errors)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, TypeError, ValueError, OSError) as error:
        print("Dependency audit failed: " + str(error), file=sys.stderr)
        sys.exit(1)
