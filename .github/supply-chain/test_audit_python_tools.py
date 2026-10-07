#!/usr/bin/env python3
# Tests of audit_python_tools.py, with a stand-in for pip-audit so that they need no network. The supply-chain workflow runs them before the audit: python3 -m unittest discover --start-directory .github/supply-chain

from __future__ import annotations

import contextlib
import io
import json
import os
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import audit_python_tools  # noqa: E402

# A stand-in for pip-audit. FAKE_PIP_AUDIT chooses how it behaves: "honest" reports the canary's vulnerabilities and finds none elsewhere, "silent" reports nothing at all, "vulnerable" also finds a vulnerability in the real files. Every call is logged as a JSON line to FAKE_PIP_AUDIT_LOG.
FAKE = textwrap.dedent(
    """\
    import json, os, sys
    arguments = sys.argv[1:]
    with open(os.environ["FAKE_PIP_AUDIT_LOG"], "a", encoding="utf-8") as log:
        log.write(json.dumps(arguments) + "\\n")
    mode = os.environ["FAKE_PIP_AUDIT"]
    if "--format" in arguments:
        vulns = [] if mode == "silent" else [{"id": "PYSEC-2026-1471", "fix_versions": ["3.1.6"], "aliases": [], "description": ""}]
        report = {"dependencies": [{"name": "jinja2", "version": "3.1.4", "vulns": vulns}], "fixes": []}
        with open(arguments[arguments.index("--output") + 1], "w", encoding="utf-8") as output:
            json.dump(report, output)
        sys.exit(1 if vulns else 0)
    if mode == "vulnerable":
        print("Found 1 known vulnerability in 1 package")
        sys.exit(1)
    print("No known vulnerabilities found")
    """
)


class CanaryTests(unittest.TestCase):
    def test_accepts_a_report_of_the_known_vulnerabilities(self) -> None:
        report = json.dumps({"dependencies": [{"name": "jinja2", "version": "3.1.4", "vulns": [{"id": "PYSEC-2026-1471"}]}], "fixes": []})
        self.assertIsNone(audit_python_tools.canary_problem(1, report))

    def test_refuses_a_report_without_them(self) -> None:
        silent = json.dumps({"dependencies": [{"name": "jinja2", "version": "3.1.4", "vulns": []}], "fixes": []})
        self.assertIn("no vulnerability", audit_python_tools.canary_problem(1, silent) or "")
        self.assertIn("status 0", audit_python_tools.canary_problem(0, silent) or "")
        self.assertIn("status 2", audit_python_tools.canary_problem(2, "") or "")
        self.assertIn("no readable JSON", audit_python_tools.canary_problem(1, "not json") or "")
        self.assertIn("no readable JSON", audit_python_tools.canary_problem(1, "[]") or "")
        other = json.dumps({"dependencies": [{"name": "requests", "version": "2.0.0", "vulns": [{"id": "X"}]}]})
        self.assertIn("no vulnerability for jinja2", audit_python_tools.canary_problem(1, other) or "")


class AuditTests(unittest.TestCase):
    def setUp(self) -> None:
        self._folder = tempfile.TemporaryDirectory()
        folder = Path(self._folder.name)
        self.root = folder / "repository"
        self.root.mkdir()
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True)
        for name in [".github/reuse/requirements.txt", ".github/reuse/build-requirements.txt", "deps/requirements-tools.txt", "notes.txt"]:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("# pins\n", encoding="utf-8")
        (self.root / "untracked-requirements.txt").write_text("# not added to Git\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(self.root), "add", ".github", "deps", "notes.txt"], check=True)
        self.fake = folder / "pip-audit"
        self.fake.write_text(f"#!{sys.executable}\n{FAKE}", encoding="utf-8")
        self.fake.chmod(self.fake.stat().st_mode | stat.S_IXUSR)
        self.log = folder / "calls.jsonl"

    def tearDown(self) -> None:
        self._folder.cleanup()

    def run_main(self, mode: str) -> tuple[int, str, list]:
        output = io.StringIO()
        with mock.patch.dict(os.environ, {"FAKE_PIP_AUDIT": mode, "FAKE_PIP_AUDIT_LOG": str(self.log)}):
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                status = audit_python_tools.main(["--pip-audit", str(self.fake), "--root", str(self.root)])
        calls = [json.loads(line) for line in self.log.read_text(encoding="utf-8").splitlines()] if self.log.exists() else []
        return status, output.getvalue(), calls

    def test_finds_the_tracked_requirements_files(self) -> None:
        self.assertEqual(
            sorted(audit_python_tools.requirement_files(self.root)),
            [".github/reuse/build-requirements.txt", ".github/reuse/requirements.txt", "deps/requirements-tools.txt"],
        )

    def test_audits_every_file_after_the_canary(self) -> None:
        status, output, calls = self.run_main("honest")
        self.assertEqual(status, 0, output)
        self.assertEqual(len(calls), 2, calls)
        canary, audit = calls
        self.assertIn(str(audit_python_tools.CANARY), canary)
        self.assertIn("--disable-pip", audit)
        self.assertIn("--require-hashes", audit)
        self.assertIn("--strict", audit)
        audited = sorted(audit[index + 1] for index, argument in enumerate(audit) if argument == "--requirement")
        self.assertEqual(audited, [".github/reuse/build-requirements.txt", ".github/reuse/requirements.txt", "deps/requirements-tools.txt"])

    def test_fails_when_a_pin_is_vulnerable(self) -> None:
        status, output, _ = self.run_main("vulnerable")
        self.assertEqual(status, 1, output)
        self.assertIn("known vulnerability", output)

    def test_refuses_to_trust_a_pip_audit_that_finds_nothing(self) -> None:
        status, output, calls = self.run_main("silent")
        self.assertEqual(status, 2, output)
        self.assertIn("cannot be trusted", output)
        self.assertEqual(len(calls), 1, "the real files must not be audited after a failed canary")

    def test_reports_a_missing_pip_audit(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            status = audit_python_tools.main(["--pip-audit", str(self.root / "no-such-program"), "--root", str(self.root)])
        self.assertEqual(status, 2)
        self.assertIn("cannot find", output.getvalue())

    def test_the_canary_pins_a_vulnerable_version_with_a_hash(self) -> None:
        lines = [line for line in audit_python_tools.CANARY.read_text(encoding="utf-8").splitlines() if not line.startswith("#")]
        self.assertEqual(lines[0], "jinja2==3.1.4 \\")
        self.assertRegex(lines[1], r"^    --hash=sha256:[0-9a-f]{64}$")
        self.assertNotIn("requirements", audit_python_tools.CANARY.name)


if __name__ == "__main__":
    unittest.main()
