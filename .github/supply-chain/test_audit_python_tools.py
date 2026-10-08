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

# A stand-in for pip-audit. FAKE_PIP_AUDIT chooses how it behaves: "honest" reports the canary's vulnerabilities and finds none elsewhere, "silent" reports nothing at all, "vulnerable" also finds a vulnerability in deps/requirements-tools.txt, and "broken" fails on that file the way pip-audit does when it cannot audit one (exit status 1, an error message, no report). Every call is logged as a JSON line to FAKE_PIP_AUDIT_LOG.
FAKE = textwrap.dedent(
    """\
    import json, os, sys
    arguments = sys.argv[1:]
    with open(os.environ["FAKE_PIP_AUDIT_LOG"], "a", encoding="utf-8") as log:
        log.write(json.dumps(arguments) + "\\n")
    mode = os.environ["FAKE_PIP_AUDIT"]
    files = [arguments[index + 1] for index, argument in enumerate(arguments) if argument == "--requirement"]
    canary = any(name.endswith("pip-audit-canary.txt") for name in files)
    if mode == "broken" and files == ["deps/requirements-tools.txt"]:
        print("ERROR:pip_audit._cli:package jinja2 has duplicate requirements", file=sys.stderr)
        sys.exit(1)
    if canary:
        vulns = [] if mode == "silent" else [{"id": "PYSEC-2026-1471", "fix_versions": ["3.1.6"], "aliases": [], "description": ""}]
        dependencies = [{"name": "jinja2", "version": "3.1.4", "vulns": vulns}]
    elif mode == "vulnerable" and files == ["deps/requirements-tools.txt"]:
        dependencies = [{"name": "cmake", "version": "4.1.0", "vulns": [{"id": "PYSEC-2026-0001", "fix_versions": ["4.1.1"], "aliases": [], "description": ""}]}]
    else:
        dependencies = [{"name": "reuse", "version": "6.2.0", "vulns": []}]
    with open(arguments[arguments.index("--output") + 1], "w", encoding="utf-8") as output:
        json.dump({"dependencies": dependencies, "fixes": []}, output)
    sys.exit(1 if any(dependency["vulns"] for dependency in dependencies) else 0)
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

    def test_audits_every_file_in_a_run_of_its_own_after_the_canary(self) -> None:
        # One run per file: two files may pin the same package at different versions, which pip-audit refuses within one run.
        status, output, calls = self.run_main("honest")
        self.assertEqual(status, 0, output)
        self.assertEqual(len(calls), 4, calls)
        canary, *audits = calls
        self.assertIn(str(audit_python_tools.CANARY), canary)
        audited = []
        for audit in audits:
            for option in ["--disable-pip", "--require-hashes", "--strict", "--format", "--output"]:
                self.assertIn(option, audit)
            files = [audit[index + 1] for index, argument in enumerate(audit) if argument == "--requirement"]
            self.assertEqual(len(files), 1, audit)
            audited += files
        self.assertEqual(sorted(audited), [".github/reuse/build-requirements.txt", ".github/reuse/requirements.txt", "deps/requirements-tools.txt"])
        self.assertIn("No known vulnerabilities", output)

    def test_fails_when_a_pin_is_vulnerable(self) -> None:
        status, output, _ = self.run_main("vulnerable")
        self.assertEqual(status, 1, output)
        self.assertIn("known vulnerability", output)
        self.assertIn("deps/requirements-tools.txt: cmake 4.1.0: PYSEC-2026-0001 (fixed in 4.1.1)", output)

    # pip-audit exits with status 1 when it cannot audit a file, too; that used to be reported as "A pinned Python tool has a known vulnerability" (found in the review of X-003).
    def test_reports_a_file_that_pip_audit_cannot_audit_as_an_error(self) -> None:
        status, output, calls = self.run_main("broken")
        self.assertEqual(status, 2, output)
        self.assertIn("could not audit deps/requirements-tools.txt", output)
        self.assertIn("duplicate requirements", output)
        self.assertNotIn("known vulnerability", output)
        self.assertEqual(len(calls), 4, "the other files are still audited")

    def test_reads_the_report_strictly(self) -> None:
        report = lambda vulns: json.dumps({"dependencies": [{"name": "cmake", "version": "4.1.0", "vulns": vulns}], "fixes": []})  # noqa: E731
        found = audit_python_tools.vulnerabilities("r.txt", 1, report([{"id": "X", "fix_versions": []}]), "")
        self.assertEqual(found, ["r.txt: cmake 4.1.0: X (fixed in no fixed release yet)"])
        self.assertEqual(audit_python_tools.vulnerabilities("r.txt", 0, report([]), ""), [])
        for status, text in [(1, None), (1, report([])), (0, report([{"id": "X"}])), (2, report([])), (1, "not json"), (1, "[]"), (1, json.dumps({"dependencies": [1]}))]:
            with self.subTest(status=status, text=text), self.assertRaises(audit_python_tools.AuditError):
                audit_python_tools.vulnerabilities("r.txt", status, text, "")

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
