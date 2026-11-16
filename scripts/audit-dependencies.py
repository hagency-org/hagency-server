#!/usr/bin/env python3
"""Enforce an unfiltered clean audit or an explicit legacy risk review."""
import argparse
import hashlib
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
    if not isinstance(report, dict) or not isinstance(report["vulnerabilities"], dict) or not isinstance(report["warnings"], dict):
        raise ValueError("malformed audit report containers")
    vulnerabilities = report["vulnerabilities"]["list"]
    count = report["vulnerabilities"]["count"]
    if not isinstance(vulnerabilities, list) or type(count) is not int or count < 0 or count != len(vulnerabilities):
        raise ValueError("inconsistent audit vulnerability count")
    if "found" in report["vulnerabilities"]:
        found = report["vulnerabilities"]["found"]
        if type(found) is not bool or found != bool(count):
            raise ValueError("inconsistent audit vulnerability flag")
    records = [("vulnerability", entry) for entry in vulnerabilities]
    for kind, entries in report["warnings"].items():
        if not isinstance(kind, str) or not isinstance(entries, list):
            raise ValueError("malformed audit warning list")
        records.extend((kind, entry) for entry in entries)
    result = {}
    for kind, entry in records:
        if not isinstance(entry, dict) or not isinstance(entry["advisory"], dict) or not isinstance(entry["package"], dict):
            raise ValueError("malformed audit finding")
        key = (entry["advisory"]["id"], kind, entry["package"]["name"], entry["package"]["version"])
        if any(not isinstance(value, str) or not value for value in key):
            raise ValueError("malformed audit finding identity")
        if key in result:
            raise ValueError("duplicate audit finding")
        # An unchanged ID/version does not imply unchanged risk. RustSec can
        # publish fixes or revise impact text without adding a new finding.
        semantic = {"advisory": entry["advisory"], "versions": entry.get("versions"), "affected": entry.get("affected")}
        result[key] = hashlib.sha256(json.dumps(semantic, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return result


def check(report, policy, lock, today):
    actual = findings(report)
    errors = []
    if policy["version"] not in (2, 3):
        raise ValueError("unsupported dependency policy")
    reviewed = date.fromisoformat(policy["reviewed_on"])
    if policy["version"] == 3:
        if policy.get("mode") != "strict-clean" or policy["findings"] != [] or "expires_on" in policy:
            raise ValueError("strict-clean policy cannot contain exceptions or an exception expiry")
        expected_settings = {"target_arch": [], "target_os": [], "severity": None, "ignore": [],
                             "informational_warnings": ["unmaintained", "unsound", "notice"]}
        settings = report.get("settings")
        if not isinstance(settings, dict) or settings != expected_settings:
            raise ValueError("strict-clean audit must include all targets, severities and informational warnings without ignores")
        if "found" not in report["vulnerabilities"]:
            raise ValueError("strict-clean audit needs a vulnerability flag")
        if reviewed > today:
            errors.append("dependency source review is future-dated")
    else:
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
    reviewed_details = {}
    for row in policy["findings"]:
        key = (row["advisory"], row["kind"], row["package"], row["version"])
        if key in accepted or not row["reason"].strip() or not row["reference"].startswith("https://"):
            raise ValueError("invalid or duplicate dependency disposition")
        if row["kind"] == "vulnerability" and row["status"] != "unresolved_upstream":
            raise ValueError("a vulnerable version cannot be described as resolved")
        fingerprint = row.get("advisory_fingerprint", "")
        if not re.fullmatch(r"[0-9a-f]{64}", fingerprint):
            raise ValueError("dependency disposition needs reviewed advisory details")
        accepted.add(key)
        reviewed_details[key] = fingerprint
    errors.extend("unreviewed finding: " + " / ".join(row) for row in sorted(set(actual) - accepted))
    errors.extend("remove stale disposition: " + " / ".join(row) for row in sorted(accepted - set(actual)))
    errors.extend("advisory details or affected/fixed ranges changed: " + " / ".join(row)
                  for row in sorted(accepted & set(actual)) if reviewed_details[row] != actual[row])
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
    if policy["version"] == 3 and result.returncode != 0:
        errors.append("strict-clean cargo-audit process did not succeed")
    print("Audit: {} unresolved vulnerability records; {} informational warnings.".format(
        report["vulnerabilities"]["count"], sum(len(rows) for rows in report["warnings"].values())))
    for row in policy["findings"]:
        print("{} {} {}: {}".format(row["status"], row["advisory"], row["package"], row["reason"]))
    for error in errors:
        print("ERROR: " + error, file=sys.stderr)
    if not errors:
        if policy["version"] == 3:
            print("Clean dependency audit passed: no vulnerabilities, warnings or exceptions.")
        else:
            print("Dependency policy satisfied until {}; unresolved risks remain recorded.".format(policy["expires_on"]))
    return bool(errors)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, TypeError, ValueError, OSError) as error:
        print("Dependency audit failed: " + str(error), file=sys.stderr)
        sys.exit(1)
