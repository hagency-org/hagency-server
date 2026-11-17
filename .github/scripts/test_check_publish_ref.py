import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("check-publish-ref.py")
ROOT = SCRIPT.resolve().parents[2]
FIRST = "a" * 40
SECOND = "b" * 40


class PublicationRefTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.reply = self.root / "reply.json"
        self.calls = self.root / "calls.jsonl"
        self.output = self.root / "output"
        fake = self.root / "gh"
        fake.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, sys\n"
            "with pathlib.Path(os.environ['HAGENCY_TEST_CALLS']).open('a') as f:\n"
            "    f.write(json.dumps(sys.argv[1:]) + '\\n')\n"
            "sys.stdout.write(pathlib.Path(os.environ['HAGENCY_TEST_REPLY']).read_text())\n"
            "sys.stderr.write(os.environ['GH_TOKEN'])\n"
            "sys.exit(int(os.environ.get('HAGENCY_TEST_RETURN_CODE', '0')))\n"
        )
        fake.chmod(0o755)
        self.env = dict(
            os.environ,
            PATH=str(self.root) + os.pathsep + os.environ.get("PATH", ""),
            GH_TOKEN="test-secret-never-print",
            GITHUB_REF="refs/heads/main",
            GITHUB_SHA=FIRST,
            GITHUB_REPOSITORY="hagency-org/hagency-server",
            GITHUB_OUTPUT=str(self.output),
            HAGENCY_TEST_CALLS=str(self.calls),
            HAGENCY_TEST_REPLY=str(self.reply),
        )
        self.respond(FIRST)

    def respond(self, revision, ref="refs/heads/main"):
        self.reply.write_text(json.dumps({"ref": ref, "object": {"type": "commit", "sha": revision}}))

    def check(self, *, success=True, publish=True):
        self.output.write_text("")
        result = subprocess.run(
            [sys.executable, str(SCRIPT)], env=self.env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=5,
        )
        self.assertEqual(result.returncode, 0 if success else 1, result.stderr)
        outputs = dict(line.split("=", 1) for line in self.output.read_text().splitlines())
        self.assertEqual(outputs["publish"], str(publish).lower())
        self.assertNotIn(self.env["GH_TOKEN"], result.stdout + result.stderr)
        return result

    def test_current_main_and_next_use_read_only_ref_lookup(self):
        for branch in ("main", "next"):
            with self.subTest(branch=branch):
                self.env["GITHUB_REF"] = f"refs/heads/{branch}"
                self.respond(FIRST, self.env["GITHUB_REF"])
                self.check()
                call = json.loads(self.calls.read_text().splitlines()[-1])
                self.assertEqual(call, ["api", "--method", "GET", f"repos/hagency-org/hagency-server/git/ref/heads/{branch}"])

    def test_stale_main_and_next_skip_publication(self):
        for branch in ("main", "next"):
            with self.subTest(branch=branch):
                self.env["GITHUB_REF"] = f"refs/heads/{branch}"
                self.respond(SECOND, self.env["GITHUB_REF"])
                result = self.check(publish=False)
                self.assertIn("Skipping stale publication", result.stdout)

    def test_old_main_push_rerun_keeps_old_context_and_is_rejected(self):
        self.env["GITHUB_EVENT_NAME"] = "push"
        self.env["GITHUB_RUN_ATTEMPT"] = "2"
        self.check()
        self.respond(SECOND)
        self.env["GITHUB_SHA"] = SECOND
        self.check()
        self.env["GITHUB_SHA"] = FIRST
        self.check(publish=False)
        self.assertEqual(len(self.calls.read_text().splitlines()), 3)

    def test_tag_and_nonmoving_branch_preserve_existing_publication_without_api(self):
        self.env["HAGENCY_TEST_RETURN_CODE"] = "1"
        self.reply.write_text("not a response")
        for ref in ("refs/tags/v1.2.3", "refs/heads/release/example"):
            with self.subTest(ref=ref):
                self.env["GITHUB_REF"] = ref
                self.check()
        self.assertFalse(self.calls.exists())

    def test_api_failure_does_not_open_gate_or_echo_credentials(self):
        self.env["HAGENCY_TEST_RETURN_CODE"] = "1"
        self.check(success=False, publish=False)

    def test_malformed_response_never_opens_gate(self):
        for payload in ("not json", "null", "[]", "{}", json.dumps({"ref": "refs/heads/main"})):
            with self.subTest(payload=payload):
                self.reply.write_text(payload)
                self.check(success=False, publish=False)

    def test_wrong_ref_object_type_and_sha_never_open_gate(self):
        for ref, kind, revision in (
            ("refs/heads/next", "commit", FIRST),
            ("refs/heads/main", "tag", FIRST),
            ("refs/heads/main", "commit", None),
            ("refs/heads/main", "commit", "truncated"),
        ):
            with self.subTest(ref=ref, kind=kind, revision=revision):
                self.reply.write_text(json.dumps({"ref": ref, "object": {"type": kind, "sha": revision}}))
                self.check(success=False, publish=False)

    def test_missing_or_malformed_context_never_opens_gate(self):
        for key, value in (("GITHUB_REF", ""), ("GITHUB_SHA", "short"), ("GITHUB_REPOSITORY", "../other?bad")):
            with self.subTest(key=key):
                previous = self.env[key]
                self.env[key] = value
                self.check(success=False, publish=False)
                self.env[key] = previous
        del self.env["GITHUB_SHA"]
        self.check(success=False, publish=False)
        self.assertFalse(self.calls.exists())

    def test_workflow_guards_entire_manifest_publication_chain(self):
        text = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}", text)
        self.assertIn("cancel-in-progress: ${{ !startsWith(github.ref, 'refs/tags/') }}", text)
        publish = text.split("\n  publish:\n", 1)[1]
        steps = publish.split("      - ")[1:]
        self.assertIn("actions/checkout@v7", steps[0])
        self.assertIn("ref: ${{ github.sha }}", steps[0])
        self.assertIn("persist-credentials: false", steps[0])
        self.assertIn("id: publish_ref", steps[1])
        self.assertIn("GH_TOKEN: ${{ github.token }}", steps[1])
        self.assertIn("python3 .github/scripts/check-publish-ref.py", steps[1])
        for step in steps[2:]:
            with self.subTest(step=step.splitlines()[0]):
                self.assertIn("if: steps.publish_ref.outputs.publish == 'true'", step)
        self.assertIn("type=raw,value=latest,enable=${{ github.ref == 'refs/heads/main' }}", publish)
        self.assertIn("type=raw,value=edge,enable=${{ github.ref == 'refs/heads/main' }}", publish)
        self.assertIn("type=semver,pattern={{version}}", publish)
        self.assertIn("type=semver,pattern={{major}}.{{minor}}", publish)
        self.assertIn("type=sha,prefix=sha-", publish)
        self.assertIn("github.event_name == 'push' && (github.ref == 'refs/heads/main' || startsWith(github.ref, 'refs/tags/v'))", publish)
        image = text.split("\n  image:\n", 1)[1].split("\n  publish:\n", 1)[0]
        self.assertIn("push-by-digest=true,name-canonical=true,push=true", image)
        self.assertNotIn("steps.publish_ref", image)

    def test_mock_workflow_runs_on_changed_paths_for_any_pr_base(self):
        text = (ROOT / ".github/workflows/publication-guard.yml").read_text()
        trigger = text.split("\njobs:\n", 1)[0]
        self.assertIn("pull_request:", trigger)
        self.assertNotIn("branches:", trigger)
        self.assertNotIn("push:", trigger)
        self.assertNotIn("workflow_dispatch:", trigger)
        self.assertIn("contents: read", trigger)
        self.assertNotIn("packages:", trigger)
        for path in (
            ".github/scripts/check-publish-ref.py",
            ".github/scripts/test_check_publish_ref.py",
            ".github/workflows/ci.yml",
            ".github/workflows/publication-guard.yml",
        ):
            self.assertIn("- '" + path + "'", trigger)
        self.assertIn("python3 -m unittest discover -s .github/scripts -p test_check_publish_ref.py", text)
        self.assertNotIn("docker/", text)
        self.assertNotIn("cargo ", text)
        self.assertNotIn("GH_TOKEN:", text)
        self.assertNotIn("upload-artifact", text)


if __name__ == "__main__":
    unittest.main()
