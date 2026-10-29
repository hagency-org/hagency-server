"""Launcher guards: no real containers, signals, credentials or account writes."""
import importlib.util
import configparser
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import local_build as builds

spec = importlib.util.spec_from_file_location("local_dev", Path(__file__).with_name("local-dev.py"))
dev = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dev)


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory()
        self.addCleanup(self.folder.cleanup)
        self.state = Path(self.folder.name)
        self.override = patch.object(dev, "STATE", self.state)
        self.override.start()
        self.addCleanup(self.override.stop)

    def test_pid_reuse_is_not_owned(self):
        (self.state / "server.json").write_text(json.dumps({"pid": 123, "identity": "original"}))
        with patch.object(dev, "identity", return_value="unrelated"), patch.object(dev.os, "killpg") as kill:
            dev.stop_process("server")
            kill.assert_not_called()

    def test_changed_process_group_is_not_signalled(self):
        with patch.object(dev, "process", return_value={"pid": 123}), \
             patch.object(dev.os, "getpgid", return_value=456), patch.object(dev.os, "killpg") as kill:
            with self.assertRaisesRegex(RuntimeError, "process group changed"):
                dev.stop_process("server")
            kill.assert_not_called()

    def test_unknown_existing_volume_is_not_initialized(self):
        with patch.object(dev.subprocess, "check_output", return_value="hagency-full-dev_database\n"):
            with self.assertRaisesRegex(RuntimeError, "restore"):
                dev.initialize()
        self.assertFalse((self.state / "compose.env").exists())

    def test_missing_config_is_not_regenerated(self):
        dev.private(self.state / "compose.env", "HAGENCY_DB_PASSWORD=retained\n")
        with self.assertRaisesRegex(RuntimeError, "Incomplete environment"):
            dev.initialize()
        self.assertIn("retained", (self.state / "compose.env").read_text())

    def test_initialize_preserves_credentials_and_uses_private_files(self):
        with patch.object(dev.subprocess, "check_output", return_value=""):
            dev.initialize()
        before = {p.name: p.read_bytes() for p in self.state.iterdir()}
        dev.initialize()
        self.assertEqual(before, {p.name: p.read_bytes() for p in self.state.iterdir()})
        self.assertTrue(all(p.stat().st_mode & 0o777 == 0o600 for p in self.state.iterdir()))
        password = (self.state / "compose.env").read_text().split("=", 1)[1].strip()
        for name in ("hagency", "palpo", "pasion"):
            self.assertIn(password, (self.state / f"{name}.toml").read_text())
        config = configparser.ConfigParser()
        config.read_string((self.state / "pasion.toml").read_text())
        self.assertTrue(config.getboolean("account", "password_registration_enabled"))
        self.assertFalse(config.getboolean("account", "password_registration_contact_required"))

    def test_listener_health_without_board_is_not_ready(self):
        for product, capabilities, expected in (
            ("hagency-server", ["project-board-v2"], False),
            ("other", ["project-board-v5"], False),
            ("hagency-server", ["project-board-v5"], True),
        ):
            with self.subTest(product=product, capabilities=capabilities), patch.object(dev, "get", side_effect=[
                b"{}", json.dumps({"product": product, "capabilities": capabilities}).encode()
            ]):
                self.assertEqual(dev.ready(), expected)

    def test_bootstrap_interruption_uses_persisted_admin(self):
        with patch.object(dev.subprocess, "check_output", side_effect=["container\n", "t\n", "t\n"]):
            self.assertTrue(dev.admin_exists())

    def test_update_refuses_non_main_without_mutating_repositories(self):
        with patch.object(dev.subprocess, "check_output", return_value="feature\n"), patch.object(dev, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "switch to main"):
                dev.update_main()
            run.assert_not_called()

    def test_update_uses_fast_forward_and_never_discards_local_changes(self):
        with patch.object(dev.subprocess, "check_output", return_value="main\n"), patch.object(dev, "run") as run:
            dev.update_main()
            self.assertEqual([call.args[0] for call in run.call_args_list],
                [["git", "fetch", "origin", "main"], ["git", "merge", "--ff-only", "origin/main"]] * 2)


class BuildIntegrityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)
        self.source = {"kind": "server", "branch": "main", "head": "main-revision", "inputs": "current-source"}
        html = '<link href="/assets/hagency-theme.css"><link href="/assets/hagency-mark.svg"><script src="/assets/app.js"></script>'
        files = {"hagency-server": "binary", "frontend/public/index.html": html,
                 "pasion/public/index.html": "pasion", "pasion/templates/app.html": html,
                 "frontend/public/assets/hagency-mark.svg": "<svg/>",
                 "frontend/public/assets/hagency-theme.css": "blue theme",
                 "frontend/public/assets/hagency-theme.js": "theme()",
                 "frontend/public/assets/app.js": "compiled()",
                 "frontend/public/assets/app.wasm": "wasm", "pasion/public/assets/auth.wasm": "auth wasm"}
        for name, value in files.items():
            path = self.folder / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)
        builds.seal(self.folder, self.source)

    def test_old_revision_and_branch_resources_are_refused(self):
        builds.verify(self.folder, self.source)
        with self.assertRaisesRegex(RuntimeError, "provenance"):
            builds.verify(self.folder, {**self.source, "head": "another-revision"})

    def test_missing_shared_theme_is_not_accepted_as_a_complete_frontend(self):
        (self.folder / "frontend/public/assets/hagency-theme.css").unlink()
        with self.assertRaisesRegex(RuntimeError, "Incomplete"):
            builds.verify(self.folder, self.source)

    def test_copied_old_assets_fail_checksums(self):
        (self.folder / "frontend/public/assets/app.js").write_text("old image assets")
        with self.assertRaisesRegex(RuntimeError, "checksum"):
            builds.verify(self.folder, self.source)

    def test_http_healthy_but_serving_other_theme_fails(self):
        html = (self.folder / "frontend/public/index.html").read_bytes()
        def get(url):
            if url in ("/login", "/auth/login", "/auth/register"):
                return html
            if url == "/assets/hagency-theme.css":
                return b"old pink theme"
            return (self.folder / ("frontend/public" + url)).read_bytes()
        with self.assertRaisesRegex(RuntimeError, "Served asset differs"):
            builds.probe(self.folder, get)

    def test_inline_dioxus_module_import_is_verified(self):
        parser = builds.References()
        parser.feed('<script type="module">import init from "/assets/app.js"; init("/assets/app.wasm")</script>')
        self.assertEqual(parser.urls, ["/assets/app.js", "/assets/app.wasm"])

    def test_generated_dioxus_dot_paths_are_normalized_and_verified(self):
        parser = builds.References()
        parser.feed('<link rel="preload" href="/./assets/app.js"><script src="/./assets/app.js"></script>')
        self.assertEqual(parser.urls, ["/assets/app.js", "/assets/app.js"])

    def test_source_change_during_build_does_not_publish_pointer(self):
        with patch.object(builds, "source_state", side_effect=[self.source, {**self.source, "inputs": "changed"}]), \
             patch.object(builds.shutil, "copy2"), patch.object(builds, "seal") as seal:
            with self.assertRaisesRegex(RuntimeError, "Source changed"):
                builds.build(self.folder, self.folder, "server", lambda *a, **kw: None)
            self.assertFalse((self.folder / "server-build.json").exists())
            seal.assert_not_called()


if __name__ == "__main__":
    unittest.main()
