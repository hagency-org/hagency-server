"""Classify both sides of a PR's file changes on the tested merge commit."""
from enum import IntEnum
from fnmatch import fnmatchcase
import os
from pathlib import Path
import subprocess


class Scope(IntEnum):
    DOCS = 0
    QUALITY = 1
    SMOKE = 2
    RESTORE = 3


def scope(path):
    def matches(*patterns):
        return any(fnmatchcase(path, pattern) for pattern in patterns)

    if matches("README.md", "README.zh-CN.md", "docs/*.md", "dev-vibe/*.md"):
        return Scope.DOCS
    if matches("*/migrations/*", "*migrate*", "*/migrations.rs", "*.sql", "diesel*.toml",
               "crates/agent-service/src/store.rs",
               "crates/agent-service/src/schema/*", "crates/agent-service/src/upgrade_tests.rs",
               "scripts/test-agent-integration.py", ".github/actions/restore-drill/*",
               ".github/workflows/deployment-smoke.yml", ".github/workflows/ci.yml",
               ".github/workflows/ci-scope.yml", ".github/scripts/ci_scope.py",
               "*backup*", "*restore*"):
        return Scope.RESTORE
    if matches("crates/*/src/*_tests.rs", "crates/*/tests/*.rs", "crates/*/src/*/tests.rs"):
        return Scope.QUALITY
    if matches("*/Cargo.toml", "*/build.rs", "*/src/*matrix*", "*/src/*auth*",
               "*/src/*session*", "*/src/*crypto*", "*/src/*e2ee*", "*/src/*pasion*"):
        return Scope.SMOKE
    if matches("crates/agent-service/src/management_*.rs", "crates/agent-service/src/api_management/*.rs",
               "crates/agent-service/src/project_board/*.rs", "crates/agent-service/src/recurring_quota*.rs",
               "crates/agent-service/src/domain_discovery.rs", "crates/agent-service/src/project_folders.rs",
               "crates/agent-service/src/reply_format.rs", "crates/agent-service/openapi/*",
               "crates/frontend/src/pages/hagency/*.rs",
               "scripts/validate-agent-openapi.py", ".github/scripts/run-agent-shards.py"):
        return Scope.QUALITY
    # New or unclassified production paths get an image and real deployment check.
    return Scope.SMOKE


if __name__ == "__main__":
    changed = subprocess.check_output(
        ["git", "diff", "--name-only", "--no-renames", "-z", "HEAD^1", "HEAD", "--"]
    ).split(b"\0")
    level = max((scope(os.fsdecode(path)) for path in changed if path), default=Scope.DOCS)
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as stream:
        for key, value in (("docs_only", level == Scope.DOCS), ("smoke", level >= Scope.SMOKE),
                           ("restore", level == Scope.RESTORE)):
            print(f"{key}={str(value).lower()}", file=stream)
    print(f"PR checks: {level.name.lower()}")
