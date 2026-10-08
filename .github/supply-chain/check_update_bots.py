#!/usr/bin/env python3
# Fails when a BayanDocs repository contains configuration for a service that opens dependency-update pull requests by itself, such as Dependabot version updates or Renovate (ADR-0017 rule 1, https://github.com/BayanDocs/docs/blob/main/adr/0017-supply-chain-and-dependency-policy.md; work package X-003).
#
# BayanDocs updates its dependencies in batched sessions instead, about once a month or when a security alert fires, following the docs repository's developer/dependency-update-runbook.md. Dependabot security alerts stay switched on as a repository setting; they open no pull requests and need no file, so a configuration file can only mean version-update pull requests.
#
# It looks, comparing names without regard to letter case, for:
#   - the configuration files that the services read (FILES below), wherever each service looks for them;
#   - a "renovate" section in package.json, which Renovate reads too;
#   - a workflow in .github/workflows that runs an update bot or acts on its pull requests (WORKFLOW_USES below).
#
# Usage: check_update_bots.py [--root DIR]   (default: the repository this script is in)
# Exit status: 0 when nothing is found, 1 when update-bot configuration is found, 2 when the check could not run.
#
# Needs Python 3.9 or later, nothing else. The tests are in test_check_update_bots.py next to this file. This folder is identical in all five BayanDocs repositories: change it in all of them together.

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import List, Optional, Sequence, Tuple

ROOT = Path(__file__).resolve().parents[2]

# Configuration files of update bots, relative to the repository root, with the service that reads each.
FILES: Tuple[Tuple[str, str], ...] = (
    (".github/dependabot.yml", "Dependabot version updates"),
    (".github/dependabot.yaml", "Dependabot version updates"),
    (".dependabot/config.yml", "Dependabot (its former configuration file)"),
    ("renovate.json", "Renovate"),
    ("renovate.json5", "Renovate"),
    (".renovaterc", "Renovate"),
    (".renovaterc.json", "Renovate"),
    (".renovaterc.json5", "Renovate"),
    (".github/renovate.json", "Renovate"),
    (".github/renovate.json5", "Renovate"),
    (".gitlab/renovate.json", "Renovate"),
    (".gitlab/renovate.json5", "Renovate"),
    (".config/renovate.json", "Renovate"),
    (".config/renovate.json5", "Renovate"),
    (".whitesource", "Mend Renovate (formerly WhiteSource)"),
    (".depfu.yml", "Depfu"),
    (".pyup.yml", "PyUp"),
    (".scala-steward.conf", "Scala Steward"),
    (".github/.scala-steward.conf", "Scala Steward"),
    (".config/.scala-steward.conf", "Scala Steward"),
)

# What a workflow names when it runs an update bot, or automates the handling of its pull requests: the Renovate action and image, and Dependabot's helper for merging its pull requests.
WORKFLOW_USES = re.compile(r"renovatebot/|renovate/renovate|dependabot/fetch-metadata", re.IGNORECASE)


class CheckError(Exception):
    """The check could not run (exit status 2)."""


def find_case_insensitive(root: Path, relative: str) -> Optional[Path]:
    """The file at `relative` below `root`, matching each part of the path without regard to letter case, or None. On a case-sensitive file system, `.github/Dependabot.yml` is a different file from `.github/dependabot.yml`; the check finds both."""
    current = root
    for part in relative.split("/"):
        try:
            entries = list(current.iterdir())
        except (FileNotFoundError, NotADirectoryError):
            return None
        matches = [entry for entry in entries if entry.name.casefold() == part.casefold()]
        if not matches:
            return None
        current = sorted(matches)[0]
    return current


def strip_yaml_comment(line: str) -> str:
    """The line without a YAML comment: from a "#" at the start or after a space. Good enough for workflow files, where "#" inside a value is rare; a comment that mentions a bot is never a finding."""
    if line.lstrip().startswith("#"):
        return ""
    return re.split(r"\s#", line, maxsplit=1)[0]


def check(root: Path) -> List[str]:
    """Every piece of update-bot configuration in the repository at `root`, described for people; empty when there is none."""
    findings = []
    for relative, service in FILES:
        found = find_case_insensitive(root, relative)
        if found is not None:
            findings.append(f"{found.relative_to(root).as_posix()}: configuration for {service}")
    package_json = find_case_insensitive(root, "package.json")
    if package_json is not None and package_json.is_file():
        try:
            manifest = json.loads(package_json.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise CheckError(f"cannot read {package_json.name}: {error}") from error
        if isinstance(manifest, dict) and any(str(key).casefold() == "renovate" for key in manifest):
            findings.append(f"{package_json.relative_to(root).as_posix()}: a \"renovate\" section, which configures Renovate")
    workflows = find_case_insensitive(root, ".github/workflows")
    if workflows is not None and workflows.is_dir():
        for workflow in sorted(workflows.iterdir()):
            if workflow.suffix.casefold() not in (".yml", ".yaml") or not workflow.is_file():
                continue
            try:
                lines = workflow.read_text(encoding="utf-8").splitlines()
            except (OSError, UnicodeDecodeError) as error:
                raise CheckError(f"cannot read {workflow.name}: {error}") from error
            for number, line in enumerate(lines, start=1):
                match = WORKFLOW_USES.search(strip_yaml_comment(line))
                if match:
                    findings.append(f"{workflow.relative_to(root).as_posix()} line {number}: runs or serves an update bot ({match.group(0)})")
    return findings


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Fails when the repository configures an update bot such as Dependabot version updates or Renovate (ADR-0017 rule 1).")
    parser.add_argument("--root", type=Path, default=ROOT, help="the repository to check (default: the one this script is in)")
    arguments = parser.parse_args(argv)
    if not arguments.root.is_dir():
        print(f"error: {arguments.root} is not a folder", file=sys.stderr)
        return 2
    try:
        findings = check(arguments.root)
    except CheckError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if findings:
        print("Update-bot configuration found. BayanDocs never uses update bots (ADR-0017 rule 1): dependencies are updated in batched sessions instead (the docs repository's developer/dependency-update-runbook.md), and Dependabot security alerts stay on as a repository setting, which needs no file. Remove:")
        for finding in findings:
            print(f"  - {finding}")
        return 1
    print(f"No update-bot configuration: none of the {len(FILES)} known configuration files, no \"renovate\" section in package.json, and no workflow that runs an update bot.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
