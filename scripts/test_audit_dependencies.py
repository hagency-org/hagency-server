import copy
from datetime import date
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("audit_dependencies", Path(__file__).with_name("audit-dependencies.py"))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class DependencyPolicyTests(unittest.TestCase):
    def setUp(self):
        self.report = {"vulnerabilities": {"count": 1, "list": [{"advisory": {"id": "RUSTSEC-2026-0387"}, "package": {"name": "crypto-bigint", "version": "0.5.5"}}]}, "warnings": {}}
        self.policy = {"version": 1, "reviewed_on": "2026-10-29", "expires_on": "2026-11-28", "reviewed_sources": {"pasion-jose": "git+https://example.test/pasion#original"}, "findings": [{"advisory": "RUSTSEC-2026-0387", "kind": "vulnerability", "package": "crypto-bigint", "version": "0.5.5", "reason": "Incompatible upstream migration; still unresolved.", "reference": "https://rustsec.org/advisories/RUSTSEC-2026-0387.html", "status": "unresolved_upstream"}]}
        self.lock = '[[package]]\nname = "pasion-jose"\nversion = "1.8.0"\nsource = "git+https://example.test/pasion#original"\n'

    def check(self, report=None, policy=None, lock=None, today=date(2026, 10, 29)):
        return audit.check(self.report if report is None else report, self.policy if policy is None else policy, self.lock if lock is None else lock, today)

    def test_exact_reviewed_risk_remains_explicit(self):
        self.assertEqual(self.check(), [])

    def test_new_advisory_or_warning_is_not_covered(self):
        report = copy.deepcopy(self.report)
        report["warnings"]["unsound"] = [{"advisory": {"id": "RUSTSEC-2026-NEW"}, "package": {"name": "other", "version": "1.0.0"}}]
        self.assertTrue(self.check(report=report))

    def test_same_id_on_new_version_requires_review(self):
        report = copy.deepcopy(self.report)
        report["vulnerabilities"]["list"][0]["package"]["version"] = "0.5.6"
        self.assertTrue(self.check(report=report))

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


if __name__ == "__main__":
    unittest.main()
