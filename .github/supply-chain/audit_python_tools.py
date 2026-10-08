#!/usr/bin/env python3
# Audits the Python tools that CI installs from hash-pinned requirements files, with pip-audit (ADR-0017, "Python tools in CI", https://github.com/BayanDocs/docs/blob/main/adr/0017-supply-chain-and-dependency-policy.md; work package X-003).
#
# It runs in two steps:
#   1. A canary. pip-audit must report the known vulnerabilities of the deliberately vulnerable pin in pip-audit-canary.txt (next to this script). If it reports none, it cannot be trusted to find any, for example because the vulnerability service it asks has changed, and the audit fails instead of passing without having checked anything.
#   2. The audit. Every file named *requirements*.txt that Git tracks in the repository is audited in a run of pip-audit of its own, without installing anything: --disable-pip reads the exact versions from the file, --require-hashes refuses a file whose requirements are not hash-pinned, and --strict fails when a requirement cannot be read. pip-audit asks PyPI's vulnerability service. One run per file, because two files may pin the same package at different versions (REUSE's and pip-audit's both pin boolean-py and license-expression, for example), which pip-audit refuses within one run. pip-audit exits with status 1 both when it finds a vulnerability and when it cannot audit a file, so the script reads its JSON report, which pip-audit writes only in the first case: a vulnerability fails the audit with status 1, and a file that pip-audit could not audit fails it with status 2, never as a vulnerability.
#
# Usage: audit_python_tools.py [--pip-audit PROGRAM] [--root DIR]
#   --pip-audit   the pip-audit to run (default: pip-audit on PATH; docs/scripts/dev-setup.sh --pip-audit installs the pinned one)
#   --root        the repository to audit (default: the one this script is in)
# Exit status: 0 when no requirements file has a known vulnerability, 1 when one has, 2 when the audit could not run, could not audit a file, or the canary failed.
#
# Needs Python 3.9 or later, Git, and pip-audit installed from pip-audit-requirements.txt (next to this script). The tests are in test_audit_python_tools.py. This folder is identical in all five BayanDocs repositories: change it in all of them together.

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import List, Optional, Sequence

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CANARY = HERE / "pip-audit-canary.txt"
# The package of the canary, which must be reported with at least one vulnerability.
CANARY_PACKAGE = "jinja2"
# How pip-audit is run on every file: from the pinned versions and hashes in the file, without pip resolving or installing anything.
AUDIT_OPTIONS = ["--require-hashes", "--disable-pip", "--progress-spinner", "off"]


class AuditError(Exception):
    """The audit could not run (exit status 2)."""


def requirement_files(root: Path) -> List[str]:
    """The files named *requirements*.txt that Git tracks below `root`, relative to it, in Git's order."""
    try:
        listed = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z", "--", "*requirements*.txt"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        raise AuditError(f"cannot list the files Git tracks in {root}: {error}") from error
    return [name for name in listed.split("\0") if name]


def canary_problem(status: int, report: str) -> Optional[str]:
    """Why the canary failed, or None if pip-audit reported a vulnerability of the canary package. `status` is pip-audit's exit status and `report` its JSON report."""
    if status != 1:
        return f"pip-audit exited with status {status} on the canary, not 1 (vulnerabilities found)"
    try:
        dependencies = json.loads(report)["dependencies"]
    except (json.JSONDecodeError, KeyError, TypeError):
        return "pip-audit wrote no readable JSON report for the canary"
    for dependency in dependencies if isinstance(dependencies, list) else []:
        if isinstance(dependency, dict) and dependency.get("name") == CANARY_PACKAGE and dependency.get("vulns"):
            return None
    return f"pip-audit reported no vulnerability for {CANARY_PACKAGE} in the canary"


def run_canary(pip_audit: str) -> None:
    """Runs pip-audit on the canary; raises AuditError unless it reports the known vulnerabilities."""
    with tempfile.TemporaryDirectory() as folder:
        report = Path(folder) / "canary.json"
        try:
            result = subprocess.run(
                [pip_audit, *AUDIT_OPTIONS, "--format", "json", "--output", str(report), "--requirement", str(CANARY)],
                capture_output=True,
                text=True,
            )
        except OSError as error:
            raise AuditError(f"cannot run {pip_audit}: {error}") from error
        text = report.read_text(encoding="utf-8") if report.exists() else ""
    problem = canary_problem(result.returncode, text)
    if problem is not None:
        raise AuditError(f"{problem}, so it cannot be trusted to find vulnerabilities in the real requirements files.\n{result.stderr.strip()}")


def vulnerabilities(name: str, status: int, report: Optional[str], stderr: str) -> List[str]:
    """The known vulnerabilities that pip-audit found in the requirements file `name`, one line each, from its exit status, its JSON report (None if it wrote none) and its error output. Raises AuditError when pip-audit could not audit the file."""
    if status not in (0, 1) or report is None:
        raise AuditError(f"pip-audit could not audit {name} (exit status {status}, no report): {stderr.strip() or 'no error output'}")
    try:
        dependencies = json.loads(report)["dependencies"]
        if not isinstance(dependencies, list):
            raise TypeError("dependencies is not a list")
        found: List[str] = []
        for dependency in dependencies:
            for vulnerability in dependency.get("vulns") or []:
                fixed = ", ".join(vulnerability.get("fix_versions") or []) or "no fixed release yet"
                line = f"{name}: {dependency['name']} {dependency['version']}: {vulnerability['id']} (fixed in {fixed})"
                if line not in found:
                    found.append(line)
    except (json.JSONDecodeError, KeyError, TypeError, AttributeError) as error:
        raise AuditError(f"pip-audit wrote no readable report for {name}: {error}") from error
    if (status == 1) != bool(found):
        raise AuditError(f"pip-audit exited with status {status} for {name}, but its report lists {len(found)} vulnerabilities: {stderr.strip()}")
    return found


def audit_file(pip_audit: str, root: Path, name: str) -> List[str]:
    """Audits one requirements file (`name`, relative to `root`) in a run of pip-audit of its own; returns its known vulnerabilities, one line each, and raises AuditError when pip-audit could not audit it."""
    with tempfile.TemporaryDirectory() as folder:
        report = Path(folder) / "report.json"
        try:
            result = subprocess.run(
                [pip_audit, *AUDIT_OPTIONS, "--strict", "--format", "json", "--output", str(report), "--requirement", name],
                cwd=root,
                capture_output=True,
                text=True,
            )
        except OSError as error:
            raise AuditError(f"cannot run {pip_audit}: {error}") from error
        text = report.read_text(encoding="utf-8") if report.exists() else None
    return vulnerabilities(name, result.returncode, text, result.stderr)


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Audits the hash-pinned Python tool requirements of the repository with pip-audit (ADR-0017).")
    parser.add_argument("--pip-audit", default="pip-audit", help="the pip-audit program (default: pip-audit on PATH)")
    parser.add_argument("--root", type=Path, default=ROOT, help="the repository to audit (default: the one this script is in)")
    arguments = parser.parse_args(argv)
    pip_audit = shutil.which(arguments.pip_audit)
    if pip_audit is None:
        print(f"error: cannot find {arguments.pip_audit}; install the pinned version with docs/scripts/dev-setup.sh --pip-audit", file=sys.stderr)
        return 2
    try:
        files = requirement_files(arguments.root)
        if not files:
            raise AuditError("Git tracks no *requirements*.txt file here, so there is nothing to audit; at least .github/supply-chain/pip-audit-requirements.txt should exist")
        run_canary(pip_audit)
        print(f"pip-audit reports the known vulnerabilities of the canary ({CANARY.name}), so it can be trusted.")
        print("Auditing, one file at a time: " + ", ".join(files))
    except AuditError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    found: List[str] = []
    errors: List[str] = []
    for name in files:
        try:
            found += audit_file(pip_audit, arguments.root, name)
        except AuditError as error:
            errors.append(str(error))
    if found:
        print("A pinned Python tool has a known vulnerability:\n" + "\n".join(f"  - {line}" for line in found), file=sys.stderr)
        print("Update it to a fixed version that is at least 24 hours old, following the security-alert procedure in the docs repository's developer/dependency-update-runbook.md.", file=sys.stderr)
    if errors:
        print("error: the audit is incomplete:\n" + "\n".join(f"  - {line}" for line in errors), file=sys.stderr)
        return 2
    if found:
        return 1
    print("No known vulnerabilities in any of them.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
