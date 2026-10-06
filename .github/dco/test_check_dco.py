#!/usr/bin/env python3
# Tests for check_dco.py. The DCO workflow runs them before the check itself; to run them on your own machine:
#   python3 -m unittest discover --start-directory .github/dco --verbose
# Each test builds a small Git repository in a temporary folder with exactly the commits it needs, so nothing depends on this repository's history or on your Git settings.

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Dict, Optional, Sequence
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_dco  # noqa: E402  (the import needs the path above)
from check_dco import Identity  # noqa: E402

# People in the tests have addresses under .test, the domain reserved for testing (RFC 6761): the check refuses the domains reserved for examples, such as example.com, as sign-offs.
PERSON = Identity("Jane Doe", "jane@doe.test")
SECOND_PERSON = Identity("Rafael Ortiz", "rafael@ortiz.test")
AGENT = Identity("Claude", "noreply@anthropic.com")
AGENTS = {"noreply@anthropic.com": "Claude"}
# The last section of the hand-off template, as a pull request description starts out.
TEMPLATE_DCO_SECTION = (
    "## Developer Certificate of Origin\n"
    "(Once the DCO check is enabled. Agents leave this for the human submitter, who adds their own line before merging.)\n"
    "Signed-off-by: <human submitter's name and email>\n"
)


def sign_off(identity: Identity) -> str:
    return f"Signed-off-by: {identity}"


def co_author(identity: Identity, key: str = "Co-authored-by") -> str:
    return f"{key}: {identity}"


class Repository:
    """A throwaway Git repository with an empty Git configuration, so that settings such as commit signing never get in the way."""

    def __init__(self, path: Path) -> None:
        self.path = path
        empty_config = path / "empty-gitconfig"
        empty_config.write_text("")
        self.env = {
            **os.environ,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": str(empty_config),
            "HOME": str(path),
        }
        self.work = path / "work"
        self.work.mkdir()
        self.git("init", "--quiet", "--initial-branch=main")
        self.commit("Initial commit", PERSON, {"README.md": "start\n"}, trailers=[sign_off(PERSON)])

    def git(self, *args: str, author: Identity = PERSON, check: bool = True) -> str:
        env = {
            **self.env,
            "GIT_AUTHOR_NAME": author.name,
            "GIT_AUTHOR_EMAIL": author.email,
            "GIT_COMMITTER_NAME": author.name,
            "GIT_COMMITTER_EMAIL": author.email,
        }
        result = subprocess.run(["git", *args], cwd=self.work, env=env, capture_output=True, text=True)
        if check and result.returncode != 0:
            raise AssertionError(f"git {' '.join(args)} failed: {result.stderr}")
        return result.stdout.strip()

    def commit(self, subject: str, author: Identity, files: Optional[Dict[str, str]] = None, trailers: Sequence[str] = (), body: str = "") -> str:
        for name, content in (files or {"changes.txt": f"{subject}\n"}).items():
            (self.work / name).write_text(content)
        message = subject + "\n\n" + (body + "\n\n" if body else "") + "\n".join(trailers) + "\n"
        message_file = self.path / "message.txt"
        message_file.write_text(message)
        self.git("add", "--all", author=author)
        self.git("commit", "--quiet", "--allow-empty", "--file", str(message_file), author=author)
        return self.head()

    def head(self) -> str:
        return self.git("rev-parse", "HEAD")

    def branch(self, name: str, start: str = "main") -> None:
        self.git("switch", "--quiet", "--create", name, start)

    def switch(self, name: str) -> None:
        self.git("switch", "--quiet", name)

    def merge(self, other: str, author: Identity, trailers: Sequence[str] = (), resolve: Optional[Dict[str, str]] = None, extra: Optional[Dict[str, str]] = None) -> str:
        """Merges `other` into the current branch with a merge commit. `resolve` writes the files that resolve a conflict; `extra` changes files in the merge commit although nothing conflicts."""
        message_file = self.path / "merge-message.txt"
        message_file.write_text(f"Merge branch '{other}'\n\n" + "\n".join(trailers) + "\n")
        result = self.git("merge", "--no-ff", "--no-commit", other, author=author, check=False)
        for name, content in {**(resolve or {}), **(extra or {})}.items():
            (self.work / name).write_text(content)
        if resolve is None and "CONFLICT" in result:
            raise AssertionError(f"unexpected merge conflict: {result}")
        self.git("add", "--all", author=author)
        self.git("commit", "--quiet", "--file", str(message_file), author=author)
        return self.head()


class DcoTestCase(unittest.TestCase):
    def setUp(self) -> None:
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.repo = Repository(Path(folder.name))
        self.base = self.repo.head()
        self.repo.branch("feature")

    def run_check(self, description: Optional[str] = None, base: Optional[str] = None) -> check_dco.Report:
        return check_dco.check(self.repo.work, self.repo.head(), [base or self.base], description, AGENTS)

    def assert_passes(self, report: check_dco.Report) -> None:
        self.assertTrue(report.ok, "\n".join(check_dco.describe(report)))

    def assert_fails(self, report: check_dco.Report, *expected: str) -> None:
        self.assertFalse(report.ok, "\n".join(check_dco.describe(report)))
        problems = "\n".join([problem for verdict in report.verdicts for problem in verdict.problems] + report.description_problems)
        for text in expected:
            self.assertIn(text, problems)


class PeopleTests(DcoTestCase):
    def test_a_commit_signed_off_by_its_author_passes(self) -> None:
        self.repo.commit("Fix a typo", PERSON, trailers=[sign_off(PERSON)])
        report = self.run_check(description="")
        self.assert_passes(report)
        self.assertEqual([verdict.kind for verdict in report.verdicts], ["person"])

    def test_an_unsigned_commit_fails(self) -> None:
        self.repo.commit("Fix a typo", PERSON)
        self.assert_fails(self.run_check(description=""), 'has no "Signed-off-by:" line with its author\'s email address, such as "Signed-off-by: Jane Doe <jane@doe.test>"')

    def test_one_unsigned_commit_among_signed_ones_fails(self) -> None:
        self.repo.commit("First", PERSON, trailers=[sign_off(PERSON)])
        unsigned = self.repo.commit("Second", PERSON)
        self.repo.commit("Third", PERSON, trailers=[sign_off(PERSON)])
        report = self.run_check(description="")
        self.assert_fails(report)
        self.assertEqual([verdict.commit.sha for verdict in report.verdicts if verdict.problems], [unsigned])

    def test_a_sign_off_by_someone_else_fails(self) -> None:
        self.repo.commit("Fix a typo", PERSON, trailers=[sign_off(SECOND_PERSON)])
        self.assert_fails(self.run_check(description=""), "has no")

    def test_only_the_email_address_must_match_the_author(self) -> None:
        # The owner's decision of 2026-10-06: the name in a sign-off may be written differently from the author's, for example shortened.
        self.repo.commit("Fix a typo", PERSON, trailers=["Signed-off-by: J. Doe <jane@doe.test>"])
        self.assert_passes(self.run_check(description=""))

    def test_the_author_s_name_with_another_email_address_fails(self) -> None:
        self.repo.commit("Fix a typo", PERSON, trailers=["Signed-off-by: Jane Doe <jane@elsewhere.test>"])
        self.assert_fails(self.run_check(description=""), "has no")

    def test_letter_case_does_not_matter(self) -> None:
        self.repo.commit("Fix a typo", PERSON, trailers=["signed-off-by: jane   DOE <Jane@Doe.TEST>"])
        self.assert_passes(self.run_check(description=""))

    def test_a_commit_made_under_an_example_address_fails(self) -> None:
        # Git settings copied from an example: the sign-off matches the author, but neither names a real person's address.
        placeholder = Identity("Your Name", "you@example.com")
        self.repo.commit("Fix a typo", placeholder, trailers=[sign_off(placeholder)])
        self.assert_fails(self.run_check(description=""), 'was made under the example address "Your Name <you@example.com>"', "git config user.email")

    def test_a_sign_off_line_outside_the_trailers_does_not_count(self) -> None:
        # Git reads trailers only from the last paragraph of a message; this line is in the middle.
        self.repo.commit("Fix a typo", PERSON, body=sign_off(PERSON) + "\n\nThe last paragraph is ordinary text.")
        self.assert_fails(self.run_check(description=""), "has no")

    def test_a_person_cannot_sign_off_for_an_agent(self) -> None:
        self.repo.commit("Fix a typo", PERSON, trailers=[sign_off(PERSON), sign_off(AGENT)])
        self.assert_fails(self.run_check(description=""), 'signed off by the AI agent "Claude <noreply@anthropic.com>"')

    def test_human_co_authors_are_covered_by_the_author_s_sign_off(self) -> None:
        # For example a review suggestion accepted on GitHub, which credits the reviewer in a "Co-authored-by:" line.
        self.repo.commit("Apply suggestions from code review", PERSON, trailers=[co_author(SECOND_PERSON), sign_off(PERSON)])
        self.assert_passes(self.run_check(description=""))

    def test_a_person_s_commit_with_an_agent_co_author_needs_only_the_person_s_sign_off(self) -> None:
        # For example Claude Code on the person's own machine, committing under the person's name.
        self.repo.commit("Add a parser", PERSON, trailers=[co_author(AGENT, "Co-Authored-By"), sign_off(PERSON)])
        report = self.run_check(description="")
        self.assert_passes(report)
        self.assertEqual(report.agent_commits, 0)

    def test_a_person_s_commit_with_an_agent_co_author_still_needs_the_person_s_sign_off(self) -> None:
        self.repo.commit("Add a parser", PERSON, trailers=[co_author(AGENT, "Co-Authored-By")])
        self.assert_fails(self.run_check(description=""), "has no")


class AgentTests(DcoTestCase):
    AGENT_TRAILERS = [co_author(AGENT, "Co-Authored-By"), "Claude-Session: https://claude.ai/code/session_example"]

    def test_an_agent_commit_with_the_submitter_s_sign_off_in_the_description_passes(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        report = self.run_check(description="## Summary\nA parser.\n\n## Developer Certificate of Origin\n" + sign_off(PERSON) + "\n")
        self.assert_passes(report)
        self.assertEqual(report.agent_commits, 1)
        self.assertEqual(report.description.people if report.description else None, [PERSON])

    def test_an_agent_commit_without_a_sign_off_in_the_description_fails(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        self.assert_fails(self.run_check(description="## Summary\nA parser.\n"), 'has no "Signed-off-by: Your Name <your email address>" line')

    def test_the_template_placeholder_is_not_a_sign_off(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        self.assert_fails(self.run_check(description=TEMPLATE_DCO_SECTION), "replace the template's placeholder")

    def test_a_sign_off_with_an_example_address_does_not_count(self) -> None:
        # The format shown in the instructions, copied without filling it in, or any other address at a domain reserved for examples.
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        for line in ["Signed-off-by: Your Name <you@example.com>", "Signed-off-by: Jane Doe <Jane@Example.ORG>", "Signed-off-by: Jane Doe <jane@mail.example.net>", "Signed-off-by: Jane Doe <jane@docs.example>"]:
            with self.subTest(line=line):
                self.assert_fails(self.run_check(description=line + "\n"), "a sign-off with an example address such as you@example.com does not count")

    def test_an_example_line_next_to_a_real_sign_off_passes(self) -> None:
        # For example a description that shows the format before the submitter's own line.
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        report = self.run_check(description="Signed-off-by: Your Name <you@example.com>\n" + sign_off(PERSON) + "\n")
        self.assert_passes(report)
        self.assertEqual(report.description.people if report.description else None, [PERSON])

    def test_the_filled_in_template_passes(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        description = TEMPLATE_DCO_SECTION.replace("<human submitter's name and email>", str(PERSON))
        self.assert_passes(self.run_check(description=description))

    def test_a_description_with_windows_line_endings_passes(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        self.assert_passes(self.run_check(description="## Summary\r\nA parser.\r\n" + sign_off(PERSON) + "\r\n"))

    def test_a_sign_off_hidden_in_an_html_comment_does_not_count(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        self.assert_fails(self.run_check(description="<!--\n" + sign_off(PERSON) + "\n-->\n"), "has no")
        self.assert_fails(self.run_check(description="Text <!-- never closed\n" + sign_off(PERSON) + "\n"), "has no")

    def test_a_sign_off_inside_other_text_does_not_count(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        self.assert_fails(self.run_check(description="- " + sign_off(PERSON) + "\n"), "has no")

    def test_a_description_signed_off_by_an_agent_fails(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        self.assert_fails(self.run_check(description=sign_off(AGENT) + "\n"), 'signed off by the AI agent', 'has no "Signed-off-by: Your Name')
        # Even next to a person's sign-off, an agent's sign-off is wrong.
        self.assert_fails(self.run_check(description=sign_off(PERSON) + "\n" + sign_off(AGENT) + "\n"), "signed off by the AI agent")

    def test_an_agent_commit_without_attribution_fails(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=["Claude-Session: https://claude.ai/code/session_example"])
        self.assert_fails(self.run_check(description=sign_off(PERSON) + "\n"), 'does not name it in a "Co-authored-by:" line')

    def test_an_agent_that_signs_off_its_commit_fails(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=[*self.AGENT_TRAILERS, sign_off(AGENT)])
        self.assert_fails(self.run_check(description=sign_off(PERSON) + "\n"), "signed off by the AI agent")

    def test_agent_and_person_commits_together(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        self.repo.commit("fix: handle empty input", PERSON, trailers=[sign_off(PERSON)])
        self.assert_passes(self.run_check(description=sign_off(PERSON) + "\n"))
        self.repo.commit("fix: one more", PERSON)
        self.assert_fails(self.run_check(description=sign_off(PERSON) + "\n"), "has no")

    def test_only_people_s_commits_need_no_description_sign_off(self) -> None:
        self.repo.commit("Fix a typo", PERSON, trailers=[sign_off(PERSON)])
        self.assert_passes(self.run_check(description=TEMPLATE_DCO_SECTION))

    def test_without_a_description_the_requirement_is_only_reported(self) -> None:
        # A local run (no --description): the commits pass, and the output says the description will need a sign-off.
        self.repo.commit("feat: add a parser", AGENT, trailers=self.AGENT_TRAILERS)
        report = self.run_check(description=None)
        self.assert_passes(report)
        self.assertIn("will need a person's", "\n".join(check_dco.describe(report)))


class MergeTests(DcoTestCase):
    def setUp(self) -> None:
        super().setUp()
        self.repo.commit("Feature work", PERSON, {"feature.txt": "feature\n"}, trailers=[sign_off(PERSON)])
        self.repo.switch("main")
        # A commit that reached main after the branch started; merging main brings it into the branch.
        self.repo.commit("Other work on main", SECOND_PERSON, {"other.txt": "other\n"}, trailers=[sign_off(SECOND_PERSON)])
        self.main = self.repo.head()
        self.repo.switch("feature")

    def test_a_clean_merge_needs_no_sign_off(self) -> None:
        # What GitHub's "Update branch" button does: a merge commit without a sign-off.
        merge = self.repo.merge("main", PERSON)
        report = self.run_check(description="", base=self.main)
        self.assert_passes(report)
        self.assertEqual({verdict.commit.sha: verdict.kind for verdict in report.verdicts}[merge], "merge")

    def test_a_clean_merge_by_an_agent_needs_no_description_sign_off(self) -> None:
        self.repo.merge("main", AGENT)
        report = self.run_check(description="", base=self.main)
        self.assert_passes(report)
        self.assertEqual(report.agent_commits, 0)

    def test_a_merge_that_resolves_a_conflict_needs_a_sign_off(self) -> None:
        self.repo.switch("main")
        self.repo.commit("Change the readme on main", SECOND_PERSON, {"README.md": "main's version\n"}, trailers=[sign_off(SECOND_PERSON)])
        main = self.repo.head()
        self.repo.switch("feature")
        self.repo.commit("Change the readme on the branch", PERSON, {"README.md": "branch's version\n"}, trailers=[sign_off(PERSON)])
        self.repo.merge("main", PERSON, resolve={"README.md": "both versions, combined by hand\n"})
        self.assert_fails(self.run_check(description="", base=main), "has no")

    def test_a_signed_off_merge_that_resolves_a_conflict_passes(self) -> None:
        self.repo.switch("main")
        self.repo.commit("Change the readme on main", SECOND_PERSON, {"README.md": "main's version\n"}, trailers=[sign_off(SECOND_PERSON)])
        main = self.repo.head()
        self.repo.switch("feature")
        self.repo.commit("Change the readme on the branch", PERSON, {"README.md": "branch's version\n"}, trailers=[sign_off(PERSON)])
        self.repo.merge("main", PERSON, trailers=[sign_off(PERSON)], resolve={"README.md": "both versions, combined by hand\n"})
        self.assert_passes(self.run_check(description="", base=main))

    def test_a_merge_with_changes_of_its_own_needs_a_sign_off(self) -> None:
        # Nothing conflicts, but the merge commit also changes a file: that change is new work.
        self.repo.merge("main", PERSON, extra={"feature.txt": "changed inside the merge commit\n"})
        self.assert_fails(self.run_check(description="", base=self.main), "has no")

    def test_commits_already_on_the_base_branch_are_not_checked(self) -> None:
        # main's history starts with commits from before the DCO check; only the pull request's own commits count.
        self.repo.switch("main")
        self.repo.commit("An old commit without a sign-off", SECOND_PERSON)
        main = self.repo.head()
        self.repo.switch("feature")
        self.repo.merge("main", PERSON)
        report = self.run_check(description="", base=main)
        self.assert_passes(report)
        self.assertEqual([verdict.kind for verdict in report.verdicts], ["person", "merge"])

    def test_a_base_branch_that_moved_on_is_excluded_too(self) -> None:
        # The event names an older base commit, but the branch merged a newer main, whose unsigned commit is not the pull request's.
        self.repo.switch("main")
        self.repo.commit("A newer commit on main without a sign-off", SECOND_PERSON)
        self.repo.git("update-ref", "refs/remotes/origin/main", self.repo.head())
        self.repo.switch("feature")
        self.repo.merge("main", PERSON)
        event = {"pull_request": {"base": {"sha": self.base, "ref": "main"}, "head": {"sha": self.repo.head()}, "body": ""}}
        head, excluded, description = check_dco.from_event(self.repo.work, self.write_event(event))
        report = check_dco.check(self.repo.work, head, excluded, description, AGENTS)
        self.assert_passes(report)
        self.assertEqual([verdict.kind for verdict in report.verdicts], ["person", "merge"])

    def write_event(self, event: dict) -> Path:
        path = self.repo.path / "event.json"
        path.write_text(json.dumps(event))
        return path


class CommandLineTests(DcoTestCase):
    def main(self, *args: str, environment: Optional[Dict[str, str]] = None) -> "tuple[int, str]":
        output = io.StringIO()
        with mock.patch.dict(os.environ, environment or {}, clear=False), contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            if environment is None:
                os.environ.pop("GITHUB_ACTIONS", None)
            status = check_dco.main([*args, "--repo", str(self.repo.work)])
        return status, output.getvalue()

    def event(self, body: Optional[str]) -> Path:
        path = self.repo.path / "event.json"
        event = {"pull_request": {"number": 1, "base": {"sha": self.base, "ref": "main"}, "head": {"sha": self.repo.head()}, "body": body}}
        path.write_text(json.dumps(event))
        return path

    def test_event_file_passes_and_fails(self) -> None:
        self.repo.commit("feat: add a parser", AGENT, trailers=[co_author(AGENT, "Co-Authored-By")])
        status, output = self.main("--event", str(self.event(sign_off(PERSON))))
        self.assertEqual(status, 0, output)
        self.assertIn("Result: every DCO rule is met.", output)
        status, output = self.main("--event", str(self.event(None)))
        self.assertEqual(status, 1, output)
        self.assertIn("Result: FAILED.", output)

    def test_local_run(self) -> None:
        self.repo.commit("Fix a typo", PERSON)
        status, output = self.main("--base", "main", "--head", "HEAD")
        self.assertEqual(status, 1, output)
        description = self.repo.path / "description.md"
        description.write_text(sign_off(PERSON) + "\n")
        self.repo.git("commit", "--quiet", "--amend", "--no-edit", "--signoff")
        status, output = self.main("--base", "main", "--head", "HEAD", "--description", str(description))
        self.assertEqual(status, 0, output)

    def test_a_missing_commit_stops_the_check(self) -> None:
        status, output = self.main("--base", "main", "--head", "0" * 40)
        self.assertEqual(status, 2, output)
        self.assertIn("is not in this clone", output)

    def test_an_event_that_is_not_a_pull_request_stops_the_check(self) -> None:
        path = self.repo.path / "push.json"
        path.write_text(json.dumps({"ref": "refs/heads/main"}))
        status, output = self.main("--event", str(path))
        self.assertEqual(status, 2, output)
        bad = self.repo.path / "bad.json"
        bad.write_text(json.dumps({"pull_request": {"base": {"sha": "--output=x", "ref": "main"}, "head": {"sha": self.repo.head()}}}))
        status, output = self.main("--event", str(bad))
        self.assertEqual(status, 2, output)
        self.assertIn("is not a commit ID", output)

    def test_wrong_arguments_are_refused(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as stop:
            check_dco.main(["--base", "main"])
        self.assertEqual(stop.exception.code, 2)

    def test_in_github_actions_problems_become_annotations_and_a_summary(self) -> None:
        # Git itself removes "<" and ">" from names; the rest of this name tries to break out of the annotation and the summary table.
        self.repo.commit("Fix a typo", Identity("50% | ::warning:: *Eve* & [link](x)", "eve@eve.test"))
        summary_file = self.repo.path / "summary.md"
        status, output = self.main("--event", str(self.event("")), environment={"GITHUB_ACTIONS": "true", "GITHUB_STEP_SUMMARY": str(summary_file)})
        self.assertEqual(status, 1, output)
        errors = [line for line in output.splitlines() if line.startswith("::")]
        self.assertEqual(len(errors), 1, output)
        self.assertTrue(errors[0].startswith("::error title=DCO::Commit "), errors[0])
        self.assertIn("50%25 |", errors[0])  # "%" is escaped, so the text cannot change the annotation
        summary = summary_file.read_text()
        self.assertIn("## DCO check", summary)
        self.assertIn("50% &#124; ::warning:: \\*Eve\\* &amp; \\[link\\](x)", summary)  # no column break, formatting or link


class AnnotationTests(unittest.TestCase):
    def test_line_breaks_and_percent_signs_are_escaped(self) -> None:
        self.assertEqual(check_dco.annotation("a%b\nc\rd"), "::error title=DCO::a%25b%0Ac%0Dd")


class AgentsFileTests(unittest.TestCase):
    def write(self, text: str) -> Path:
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        path = Path(folder.name) / "agents.txt"
        path.write_text(text, encoding="utf-8")
        return path

    def test_comments_blank_lines_and_letter_case(self) -> None:
        agents = check_dco.load_agents(self.write("# AI agents\n\n Bot@Example.COM  # Example bot\nother@example.org\n"))
        self.assertEqual(agents, {"bot@example.com": "Example bot", "other@example.org": "other@example.org"})

    def test_an_entry_that_is_not_an_email_address_is_refused(self) -> None:
        with self.assertRaisesRegex(check_dco.CheckError, "line 2"):
            check_dco.load_agents(self.write("# AI agents\nClaude\n"))

    def test_an_empty_list_is_refused(self) -> None:
        with self.assertRaisesRegex(check_dco.CheckError, "no agent identities"):
            check_dco.load_agents(self.write("# nothing yet\n"))

    def test_this_repository_s_list_is_valid_and_names_claude(self) -> None:
        self.assertIn("noreply@anthropic.com", check_dco.load_agents(check_dco.AGENTS_FILE))


class IdentityTests(unittest.TestCase):
    def test_parsing(self) -> None:
        self.assertEqual(check_dco.parse_identity(" Jane Doe <jane@doe.test> "), PERSON)
        for value in ["<human submitter's name and email>", "Jane Doe", "<jane@example.com>", "Jane Doe <jane at example.com>", "Jane <jane@example.com> (owner)"]:
            self.assertIsNone(check_dco.parse_identity(value), value)

    def test_addresses_reserved_for_examples(self) -> None:
        for email in ["you@example.com", "YOU@EXAMPLE.COM", "a@example.net", "a@example.org", "a@mail.example.com", "a@example", "a@docs.example", "a@example.com."]:
            self.assertTrue(check_dco.is_example_address(email), email)
        for email in ["noreply@anthropic.com", "jane@doe.test", "a@example.co", "a@examples.com", "a@myexample.com", "a@notexample.org", "a@example.com.au", "a@example.community"]:
            self.assertFalse(check_dco.is_example_address(email), email)

    def test_only_email_addresses_are_compared(self) -> None:
        self.assertTrue(Identity("Jane Doe", "Jane@Doe.TEST").same_email(Identity("J. Doe", "jane@doe.test")))
        self.assertFalse(Identity("Jane Doe", "jane@doe.test").same_email(Identity("Jane Doe", "jane@elsewhere.test")))


if __name__ == "__main__":
    unittest.main()
