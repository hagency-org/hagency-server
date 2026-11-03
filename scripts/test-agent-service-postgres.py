#!/usr/bin/env python3
"""Run Agent service tests in an isolated database, never the application DB."""
import os
import subprocess
import sys
import uuid
from urllib.parse import quote

container = os.environ.get("HAGENCY_TEST_POSTGRES_CONTAINER", "hagency-server-postgres-1")
port = os.environ.get("HAGENCY_TEST_POSTGRES_PORT", "55438")
name = "hagency_agent_test_" + uuid.uuid4().hex[:12]
password = subprocess.check_output(["docker", "exec", container, "printenv", "POSTGRES_PASSWORD"], text=True).strip()
user = subprocess.check_output(["docker", "exec", container, "printenv", "POSTGRES_USER"], text=True).strip()
def sql(statement):
    subprocess.run(["docker", "exec", container, "psql", "-U", user, "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-c", statement], check=True)
sql(f"CREATE DATABASE {name}")
usage_name = name + "_usage"
sql(f"CREATE DATABASE {usage_name}")
env = os.environ.copy()
env.setdefault("RUST_MIN_STACK", "16777216")
env["HAGENCY_AGENT_TEST_DATABASE_URL"] = "postgres://" + quote(user, safe="") + ":" + quote(password, safe="") + "@127.0.0.1:" + port + "/" + name
env["HAGENCY_USAGE_TEST_DATABASE_URL"] = env["HAGENCY_AGENT_TEST_DATABASE_URL"].rsplit("/", 1)[0] + "/" + usage_name
try:
    result = subprocess.run(["cargo", "test", "--locked", "-p", "hagency-agent-service", "--offline", *sys.argv[1:], "--", "--include-ignored", "--test-threads=1"], env=env)
finally:
    sql(f"DROP DATABASE {usage_name}")
    sql(f"DROP DATABASE {name}")
raise SystemExit(result.returncode)
