#!/usr/bin/env python3
# Tests of check_update_bots.py. The supply-chain workflow runs them before the check: python3 -m unittest discover --start-directory .github/supply-chain

from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_update_bots  # noqa: E402


class UpdateBotTests(unittest.TestCase):
    def setUp(self) -> None:
        self._folder = tempfile.TemporaryDirectory()
        self.root = Path(self._folder.name)

    def tearDown(self) -> None:
        self._folder.cleanup()

    def write(self, relative: str, text: str = "") -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def run_main(self) -> tuple[int, str]:
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            status = check_update_bots.main(["--root", str(self.root)])
        return status, output.getvalue()

    def test_passes_a_repository_without_update_bots(self) -> None:
        self.write(".github/workflows/ci.yml", "jobs:\n  test:\n    steps:\n      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n")
        self.write(".github/dco/agents.txt")
        self.write("package.json", json.dumps({"name": "x", "dependencies": {"react": "19.3.0"}}))
        self.assertEqual(check_update_bots.check(self.root), [])
        status, output = self.run_main()
        self.assertEqual(status, 0, output)
        self.assertIn("No update-bot configuration", output)

    def test_fails_on_every_known_configuration_file(self) -> None:
        for relative, service in check_update_bots.FILES:
            with self.subTest(relative=relative):
                self.write(relative, "version: 2\n")
                findings = check_update_bots.check(self.root)
                self.assertEqual(findings, [f"{relative}: configuration for {service}"])
                status, output = self.run_main()
                self.assertEqual(status, 1, output)
                self.assertIn(relative, output)
                (self.root / relative).unlink()

    def test_finds_files_whatever_their_letter_case(self) -> None:
        self.write(".GitHub/Dependabot.YML", "version: 2\n")
        self.write("Renovate.Json", "{}\n")
        findings = check_update_bots.check(self.root)
        self.assertEqual(len(findings), 2, findings)
        self.assertTrue(any(finding.startswith(".GitHub/Dependabot.YML:") for finding in findings), findings)
        self.assertTrue(any(finding.startswith("Renovate.Json:") for finding in findings), findings)

    def test_fails_on_a_renovate_section_in_package_json(self) -> None:
        self.write("package.json", json.dumps({"name": "x", "renovate": {"extends": ["config:recommended"]}}))
        self.assertEqual(check_update_bots.check(self.root), ['package.json: a "renovate" section, which configures Renovate'])

    def test_fails_on_workflows_that_run_an_update_bot(self) -> None:
        self.write(".github/workflows/renovate.yml", "jobs:\n  renovate:\n    steps:\n      - uses: renovatebot/github-action@0123456789abcdef0123456789abcdef01234567 # v46\n")
        self.write(".github/workflows/automerge.yaml", "jobs:\n  merge:\n    steps:\n      - uses: dependabot/fetch-metadata@0123456789abcdef0123456789abcdef01234567\n")
        self.write(".github/workflows/image.yml", "jobs:\n  run:\n    container:\n      image: ghcr.io/renovatebot/renovate:41\n")
        findings = check_update_bots.check(self.root)
        self.assertEqual(len(findings), 3, findings)
        self.assertTrue(all(" line " in finding for finding in findings), findings)

    def test_ignores_comments_and_other_files_that_mention_bots(self) -> None:
        self.write(".github/workflows/ci.yml", "# No Dependabot or Renovate here; see renovatebot/github-action for what is not allowed.\njobs: {}  # dependabot/fetch-metadata is not used\n")
        self.write("docs/renovate.json", "{}\n")
        self.write(".github/workflows/notes.txt", "uses: renovatebot/github-action\n")
        self.assertEqual(check_update_bots.check(self.root), [])

    def test_cannot_run_on_an_unreadable_package_json(self) -> None:
        self.write("package.json", "{ not json")
        status, output = self.run_main()
        self.assertEqual(status, 2, output)
        self.assertIn("cannot read package.json", output)

    def test_this_repository_has_no_update_bots(self) -> None:
        self.assertEqual(check_update_bots.check(check_update_bots.ROOT), [])


if __name__ == "__main__":
    unittest.main()
