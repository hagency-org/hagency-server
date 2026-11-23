import unittest
from ci_scope import Scope, scope


class PullRequestScopeTests(unittest.TestCase):
    def test_routes(self):
        examples = {
            Scope.DOCS: ["docs/guide.md", "dev-vibe/notes.md", "README.md"],
            Scope.QUALITY: ["crates/agent-service/src/management_agent_budget.rs",
                            "crates/agent-service/src/project_board/query.rs",
                            "crates/agent-service/src/transport_file_delivery_tests.rs"],
            Scope.SMOKE: ["Cargo.lock", "Dockerfile", "xtask/src/assets.rs",
                          "crates/backend/src/pasion.rs", "crates/frontend/src/pages/login.rs",
                          "config/examples/hagency.toml", "tests/docker-smoke.mjs",
                          "crates/agent-service/src/management_agent_matrix.rs", "new/component.rs"],
            Scope.RESTORE: ["deploy/databases.sql", "crates/agent-service/migrations/inbox/v2/up.sql",
                            "crates/agent-service/src/migrations.rs", "scripts/backup.sh",
                            "scripts/test-agent-integration.py", ".github/actions/restore-drill/action.yml"],
        }
        for expected, paths in examples.items():
            for path in paths:
                with self.subTest(path=path):
                    self.assertEqual(scope(path), expected)

    def test_mixed_changes_keep_the_highest_required_check(self):
        paths = ["docs/guide.md", "crates/agent-service/src/management_agents.rs",
                 "crates/agent-service/src/migrations.rs"]
        self.assertEqual(max(map(scope, paths)), Scope.RESTORE)
