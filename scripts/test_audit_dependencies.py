import copy
from datetime import date, datetime, timezone
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("audit_dependencies", Path(__file__).with_name("audit-dependencies.py"))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class DependencyPolicyTests(unittest.TestCase):
    def setUp(self):
        self.report = {"vulnerabilities": {"count": 1, "list": [{"advisory": {"id": "RUSTSEC-2026-0387"}, "package": {"name": "crypto-bigint", "version": "0.5.5"}}]}, "warnings": {}}
        self.policy = {"version": 2, "reviewed_on": "2026-10-29", "expires_on": "2026-11-28", "reviewed_sources": {"pasion-jose": "git+https://example.test/pasion#original"}, "findings": [{"advisory": "RUSTSEC-2026-0387", "kind": "vulnerability", "package": "crypto-bigint", "version": "0.5.5", "reason": "Incompatible upstream migration; still unresolved.", "reference": "https://rustsec.org/advisories/RUSTSEC-2026-0387.html", "status": "unresolved_upstream"}]}
        self.policy["findings"][0]["advisory_fingerprint"] = next(iter(audit.findings(self.report).values()))
        self.lock = '[[package]]\nname = "pasion-jose"\nversion = "1.8.0"\nsource = "git+https://example.test/pasion#original"\n'

    def check(self, report=None, policy=None, lock=None, today=date(2026, 10, 29)):
        return audit.check(self.report if report is None else report, self.policy if policy is None else policy, self.lock if lock is None else lock, today)

    def test_exact_reviewed_risk_remains_explicit(self):
        self.assertEqual(self.check(), [])

    def test_review_date_does_not_depend_on_utc_runner_calendar(self):
        now = datetime(2026, 10, 29, 16, 21, tzinfo=timezone.utc)
        self.assertEqual(audit.review_date({"timezone": "Asia/Shanghai"}, now), date(2026, 10, 30))
        self.assertEqual(audit.review_date({"timezone": "UTC"}, now), date(2026, 10, 29))

    def test_new_advisory_or_warning_is_not_covered(self):
        report = copy.deepcopy(self.report)
        report["warnings"]["unsound"] = [{"advisory": {"id": "RUSTSEC-2026-NEW"}, "package": {"name": "other", "version": "1.0.0"}}]
        self.assertTrue(self.check(report=report))

    def test_same_id_on_new_version_requires_review(self):
        report = copy.deepcopy(self.report)
        report["vulnerabilities"]["list"][0]["package"]["version"] = "0.5.6"
        self.assertTrue(self.check(report=report))

    def test_existing_advisory_new_patch_range_requires_review(self):
        report = copy.deepcopy(self.report)
        report["vulnerabilities"]["list"][0]["versions"] = {"patched": [">=0.7.6"], "unaffected": []}
        self.assertTrue(any("ranges changed" in error for error in self.check(report=report)))

    def test_existing_advisory_impact_change_requires_review(self):
        report = copy.deepcopy(self.report)
        report["vulnerabilities"]["list"][0]["advisory"]["description"] = "A broader vulnerable operation was discovered"
        self.assertTrue(any("details" in error for error in self.check(report=report)))

    def test_unreviewed_details_cannot_be_omitted(self):
        policy = copy.deepcopy(self.policy)
        del policy["findings"][0]["advisory_fingerprint"]
        with self.assertRaises(ValueError):
            self.check(policy=policy)

    def test_fixed_finding_requires_removing_stale_disposition(self):
        self.assertTrue(self.check(report={"vulnerabilities": {"count": 0, "list": []}, "warnings": {}}))

    def test_source_changes_cannot_reuse_old_call_path_review(self):
        self.assertTrue(self.check(lock=self.lock.replace("#original", "#changed")))

    def test_expiry_future_date_and_unbounded_review_fail(self):
        self.assertTrue(self.check(today=date(2026, 11, 29)))
        self.assertTrue(self.check(today=date(2026, 10, 28)))
        policy = copy.deepcopy(self.policy)
        policy["expires_on"] = "2027-01-01"
        self.assertTrue(self.check(policy=policy))

    def test_malformed_audit_is_not_a_clean_scan(self):
        with self.assertRaises(ValueError):
            report = copy.deepcopy(self.report)
            report["vulnerabilities"]["count"] = 0
            self.check(report=report)

    def test_resolved_label_cannot_hide_a_vulnerable_version(self):
        policy = copy.deepcopy(self.policy)
        policy["findings"][0]["status"] = "resolved"
        with self.assertRaises(ValueError):
            self.check(policy=policy)

    def clean_policy(self):
        policy = copy.deepcopy(self.policy)
        policy.update(version=3, mode="strict-clean", findings=[])
        del policy["expires_on"]
        return policy

    def clean_report(self):
        return {"vulnerabilities": {"count": 0, "found": False, "list": []}, "warnings": {},
                "settings": {"target_arch": [], "target_os": [], "severity": None, "ignore": [],
                             "informational_warnings": ["unmaintained", "unsound", "notice"]}}

    def test_clean_policy_has_no_exception_expiry(self):
        clean = self.clean_report()
        self.assertEqual(self.check(report=clean, policy=self.clean_policy(), today=date(2027, 1, 1)), [])

    def test_clean_policy_rejects_every_new_advisory_and_warning(self):
        vulnerability = self.clean_report()
        vulnerability["vulnerabilities"] = self.report["vulnerabilities"] | {"found": True}
        self.assertTrue(self.check(report=vulnerability, policy=self.clean_policy()))
        warning = self.clean_report()
        warning["warnings"] = {"unmaintained": [self.report["vulnerabilities"]["list"][0]]}
        self.assertTrue(self.check(report=warning, policy=self.clean_policy()))

    def test_clean_policy_cannot_accept_a_waiver_or_expiry(self):
        policy = self.clean_policy()
        policy["findings"] = self.policy["findings"]
        with self.assertRaises(ValueError):
            self.check(policy=policy)
        policy = self.clean_policy()
        policy["expires_on"] = "2027-01-01"
        with self.assertRaises(ValueError):
            self.check(policy=policy)

    def test_clean_policy_still_rejects_source_drift_and_future_review(self):
        clean = self.clean_report()
        self.assertTrue(self.check(report=clean, policy=self.clean_policy(), lock=self.lock.replace("#original", "#changed")))
        self.assertTrue(self.check(report=clean, policy=self.clean_policy(), today=date(2026, 10, 28)))

    def test_clean_policy_requires_review_for_every_git_package(self):
        for source in ("git+https://example.test/pasion#different", "git+https://example.test/other#revision"):
            with self.subTest(source=source):
                lock = self.lock + '\n[[package]]\nname = "another-package"\nversion = "1.0.0"\nsource = "' + source + '"\n'
                policy = self.clean_policy()
                self.assertTrue(self.check(report=self.clean_report(), policy=policy, lock=lock))
                policy["reviewed_sources"]["another-package"] = source
                self.assertEqual(self.check(report=self.clean_report(), policy=policy, lock=lock), [])
                self.assertTrue(self.check(report=self.clean_report(), policy=policy, lock=lock.replace(source, source + "changed")))

    def test_clean_policy_does_not_require_git_review_for_registry_or_workspace_packages(self):
        lock = self.lock + '\n[[package]]\nname = "registry-package"\nversion = "1.0.0"\nsource = "registry+https://example.test/index"\n\n[[package]]\nname = "workspace-package"\nversion = "1.0.0"\n'
        self.assertEqual(self.check(report=self.clean_report(), policy=self.clean_policy(), lock=lock), [])

    def test_malformed_empty_audit_cannot_pass_as_clean(self):
        for field, value in (("list", {}), ("list", ""), ("count", False), ("count", 0.0),
                             ("count", -1), ("found", True), ("found", 0)):
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                report = self.clean_report()
                report["vulnerabilities"][field] = value
                self.check(report=report, policy=self.clean_policy())
        for warnings in ({"unsound": {}}, {"notice": ""}, [], None):
            with self.subTest(warnings=warnings), self.assertRaises(ValueError):
                report = self.clean_report()
                report["warnings"] = warnings
                self.check(report=report, policy=self.clean_policy())

    def test_filtered_or_incomplete_scan_cannot_pass_as_clean(self):
        for field, value in (("ignore", ["RUSTSEC-2023-0071"]), ("target_arch", ["aarch64"]),
                             ("target_os", ["linux"]), ("severity", "critical"),
                             ("informational_warnings", ["notice"]), ("unknown_filter", True)):
            with self.subTest(field=field), self.assertRaises(ValueError):
                report = self.clean_report()
                report["settings"][field] = value
                self.check(report=report, policy=self.clean_policy())
        for field in ("settings", "vulnerabilities"):
            with self.subTest(missing=field), self.assertRaises((ValueError, KeyError)):
                report = self.clean_report()
                del report[field]
                self.check(report=report, policy=self.clean_policy())

    def test_malformed_finding_identity_fails_closed(self):
        for value in (None, {}, {"advisory": {}, "package": {}},
                      {"advisory": {"id": []}, "package": {"name": "rsa", "version": "0.9.10"}}):
            with self.subTest(value=value), self.assertRaises((ValueError, KeyError)):
                report = self.clean_report()
                report["warnings"] = {"unmaintained": [value]}
                self.check(report=report, policy=self.clean_policy())

    def test_clean_report_requires_successful_audit_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "security").mkdir()
            (root / "Cargo.lock").write_text(self.lock)
            policy = self.clean_policy() | {"timezone": "UTC"}
            (root / "security/dependency-policy.json").write_text(json.dumps(policy))
            for returncode in (0, 1):
                with self.subTest(returncode=returncode):
                    completed = subprocess.CompletedProcess([], returncode, json.dumps(self.clean_report()), "")
                    output = io.StringIO()
                    with patch.object(audit, "ROOT", root), patch.object(audit.sys, "argv", ["audit-dependencies.py"]), \
                         patch.object(audit, "review_date", return_value=date(2026, 10, 29)), \
                         patch.object(audit.subprocess, "run", return_value=completed), \
                         patch.object(audit.sys, "stdout", output), patch.object(audit.sys, "stderr", io.StringIO()):
                        self.assertEqual(audit.main(), bool(returncode))
                    self.assertEqual("Clean dependency audit passed" in output.getvalue(), returncode == 0)


if __name__ == "__main__":
    unittest.main()
