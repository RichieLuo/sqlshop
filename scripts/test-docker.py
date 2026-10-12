#!/usr/bin/env python3
"""Manage this project's isolated database fixtures and run real driver tests."""
import argparse
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
COMPOSE = ["docker", "compose", "--env-file", str(ROOT / ".env.test"), "-f", str(ROOT / "compose.test.yml")]
SERVICES = ("mysql", "postgres", "sqlserver")


def run(args, **kwargs):
    subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def test_password():
    for line in (ROOT / ".env.test").read_text().splitlines():
        key, separator, value = line.partition("=")
        if separator and key.strip() == "SQLSHOP_DOCKER_PASSWORD":
            return value.strip()
    raise SystemExit("Set SQLSHOP_DOCKER_PASSWORD in .env.test first.")


def seed_sqlserver():
    sql = (ROOT / "tests/docker/sqlserver.sql").read_text()
    # The password expands inside the container; it never appears in argv/output.
    run(COMPOSE + ["exec", "-T", "sqlserver", "sh", "-c",
        'exec /opt/mssql-tools18/bin/sqlcmd -S localhost -U sa -P "$MSSQL_SA_PASSWORD" -C -b'],
        input=sql, text=True)


def test_service(service, suite="databases"):
    if service == "sqlserver":
        seed_sqlserver()
    env = dict(os.environ)
    prefix, port, user = {
        "mysql": ("MYSQL", "23306", "root"),
        "postgres": ("PG", "25432", "postgres"),
        "sqlserver": ("MSSQL", "21433", "sa"),
    }[service]
    for key, value in {
        "HOST": "127.0.0.1", "PORT": port, "USER": user,
        "PASSWORD": test_password(), "DATABASE": "sqlshop_test",
    }.items():
        env[f"SQLSHOP_TEST_{prefix}_{key}"] = value
    env["SQLSHOP_TEST_SECOND_DATABASE"] = "sqlshop_other"
    # These images generate self-signed server certificates on first startup.
    env["SQLSHOP_TEST_SELF_SIGNED_TLS"] = "1" if service != "postgres" else "0"
    print(f"Testing {service} on 127.0.0.1:{port}", flush=True)
    run(["cargo", "test", "--locked", "--test", suite, service, "--", "--ignored", "--exact"], env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("up", "test", "test-edit", "down", "status"))
    parser.add_argument("services", nargs="*", metavar="SERVICE")
    args = parser.parse_args()
    services = args.services or list(SERVICES)
    if any(service not in SERVICES for service in services):
        parser.error("Services must be mysql, postgres, or sqlserver.")
    if args.command == "up":
        run(COMPOSE + ["up", "-d", "--wait", "--wait-timeout", "360"] + services)
    elif args.command in ("test", "test-edit"):
        failed = []
        for service in services:
            try:
                test_service(service, "table_edit" if args.command == "test-edit" else "databases")
            except subprocess.CalledProcessError:
                failed.append(service)
        if failed:
            raise SystemExit("Failed database tests: " + ", ".join(failed))
    elif args.command == "down":
        run(COMPOSE + ["down", "--volumes"])
    else:
        run(COMPOSE + ["ps"])


if __name__ == "__main__":
    main()
