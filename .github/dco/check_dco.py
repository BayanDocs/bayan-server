#!/usr/bin/env python3
# Checks the Developer Certificate of Origin (DCO) rules of BayanDocs on the commits of a pull request. The rules are explained in CONTRIBUTING.md ("Developer Certificate of Origin") and were decided in ADR-0003 §5 (https://github.com/BayanDocs/docs/blob/main/adr/0003-licensing-and-contribution-model.md):
#
#   1. A commit written by a person carries a "Signed-off-by:" line with its author's email address (`git commit --signoff` adds it). The name in that line may be written differently from the author's (the owner's decision of 2026-10-06).
#   2. A commit written by an AI agent (its author is listed in agents.txt next to this script) names the agent in a "Co-authored-by:" line and is not signed off, because only a person can certify the DCO. The pull request description must then contain a "Signed-off-by:" line from the person who submits the work.
#   3. Nobody signs off in the name of an agent, neither in a commit nor in the description.
#   4. A merge commit that only joins two branches, exactly as Git merges them by itself, adds nothing of its own and needs no sign-off. Any other merge commit (one that resolves a conflict or changes something) is checked like a normal commit.
#   5. A sign-off names a real person's email address. An address at a domain reserved for examples (example.com, example.net, example.org, or one ending in .example) never counts, so a placeholder such as "you@example.com", copied from the instructions without filling it in, cannot pass for a sign-off. A commit whose author has such an address cannot be signed off at all.
#   Co-authors named in "Co-authored-by:" lines are credited, not checked: the author's sign-off covers the whole commit.
#
# Usage:
#   check_dco.py --event FILE                              in GitHub Actions; FILE is the pull_request event ($GITHUB_EVENT_PATH)
#   check_dco.py --base REV --head REV [--description FILE]  on your own machine, for example: --base origin/main --head HEAD
# Exit status: 0 when every rule is met, 1 when a rule is broken, 2 when the check could not run.
#
# Needs Python 3.9 or later and Git 2.38 or later, nothing else. The tests are in test_check_dco.py next to this file. This folder is identical in all five BayanDocs repositories: change it in all of them together.

from __future__ import annotations

import argparse
import html
import json
import os
import re
import subprocess
import sys
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Sequence, Tuple

AGENTS_FILE = Path(__file__).resolve().with_name("agents.txt")

# Trailer keys, compared without regard to letter case ("Co-Authored-By" and "Co-authored-by" are the same trailer).
SIGN_OFF = "signed-off-by"
CO_AUTHOR = "co-authored-by"

EMAIL = r"[^<>\s@]+@[^<>\s@]+"
# An identity as Git writes it: "Name <email>". The name must not be empty.
IDENTITY = re.compile(r"(?P<name>[^<>]*?)\s*<(?P<email>" + EMAIL + r")>")
# A sign-off in a pull request description: a line of its own, like the sign-off of a commit.
DESCRIPTION_SIGN_OFF = re.compile(r"\s*signed-off-by:(?P<value>.*)", re.IGNORECASE)
# Hidden text in a description (an HTML comment, also one that is never closed, which hides everything after it).
HTML_COMMENT = re.compile(r"<!--.*?(?:-->|\Z)", re.DOTALL)
OBJECT_ID = re.compile(r"[0-9a-f]{40}(?:[0-9a-f]{24})?")  # SHA-1 or SHA-256
GIT_MINIMUM = (2, 38)  # `git merge-tree --write-tree`
# Domains reserved for examples in documentation (RFC 2606 and RFC 6761), together with all their subdomains. No person has an email address there.
EXAMPLE_DOMAINS = ("example.com", "example.net", "example.org", "example")


class CheckError(Exception):
    """The check could not run (exit status 2): a broken agents.txt, a missing commit, an old Git."""


@dataclass(frozen=True)
class Identity:
    name: str
    email: str

    def __str__(self) -> str:
        return f"{self.name} <{self.email}>"

    def same_email(self, other: Identity) -> bool:
        """Same email address, ignoring letter case. Names are not compared: a sign-off may write its author's name differently."""
        return self.email.casefold() == other.email.casefold()


def parse_identity(value: str) -> Optional[Identity]:
    """Reads "Name <email>"; returns None for anything else, such as the template's "<human submitter's name and email>"."""
    match = IDENTITY.fullmatch(value.strip())
    if match is None or not match.group("name").strip():
        return None
    return Identity(match.group("name").strip(), match.group("email"))


def load_agents(path: Path) -> Dict[str, str]:
    """Reads agents.txt: one email address per line, optionally followed by a "#" comment that names the agent. Returns {email in lower case: comment}."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise CheckError(f"cannot read {path}: {error.strerror}") from None
    agents: Dict[str, str] = {}
    for number, line in enumerate(lines, start=1):
        entry, _, comment = line.partition("#")
        entry = entry.strip()
        if not entry:
            continue
        if not re.fullmatch(EMAIL, entry):
            raise CheckError(f"{path.name}, line {number}: {entry!r} is not an email address")
        agents[entry.casefold()] = comment.strip() or entry
    if not agents:
        raise CheckError(f"{path.name} lists no agent identities")
    return agents


def is_agent(identity: Identity, agents: Dict[str, str]) -> bool:
    return identity.email.casefold() in agents


def is_example_address(email: str) -> bool:
    """True for an address at a domain reserved for examples, such as the "you@example.com" of the instructions, or at one of its subdomains."""
    domain = email.rpartition("@")[2].rstrip(".").casefold()
    return any(domain == reserved or domain.endswith("." + reserved) for reserved in EXAMPLE_DOMAINS)


# ---------------------------------------------------------------------------
# Reading the commits with Git
# ---------------------------------------------------------------------------


def git(repo: Path, *args: str, ok: Sequence[int] = (0,)) -> subprocess.CompletedProcess:
    try:
        result = subprocess.run(["git", *args], cwd=repo, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    except OSError as error:
        raise CheckError(f"cannot run git: {error.strerror}") from None
    if result.returncode not in ok:
        message = result.stderr.decode("utf-8", "replace").strip().splitlines()
        raise CheckError(f"git {args[0]} failed: {message[-1] if message else f'exit status {result.returncode}'}")
    return result


def git_text(repo: Path, *args: str) -> str:
    return git(repo, *args).stdout.decode("utf-8", "replace")


def require_git(repo: Path) -> None:
    found = re.search(r"(\d+)\.(\d+)", git_text(repo, "version"))
    if found is None or (int(found.group(1)), int(found.group(2))) < GIT_MINIMUM:
        raise CheckError("Git {}.{} or later is needed".format(*GIT_MINIMUM))


def resolve(repo: Path, revision: str) -> str:
    """The full object ID of a commit, or a CheckError if this clone does not have it."""
    result = git(repo, "rev-parse", "--verify", "--quiet", "--end-of-options", f"{revision}^{{commit}}", ok=(0, 1))
    object_id = result.stdout.decode("ascii", "replace").strip()
    if result.returncode != 0 or not OBJECT_ID.fullmatch(object_id):
        raise CheckError(f"commit {revision} is not in this clone (in CI, check out the full history: fetch-depth: 0)")
    return object_id


@dataclass
class Commit:
    sha: str
    parents: List[str]
    author: Identity
    trailers: List[Tuple[str, str]]  # (key in lower case, value), in the order Git reports them

    def trailer_values(self, key: str) -> List[str]:
        return [value for found, value in self.trailers if found == key]


def read_commit(repo: Path, sha: str) -> Commit:
    # Git parses the trailers itself, by the same rules that `git commit --signoff` writes them: only the last paragraph of the message counts, so a "Signed-off-by:" line in the middle of a message is not a sign-off. Fields are separated by the ASCII unit separator.
    fields = git_text(repo, "show", "--no-patch", "--encoding=UTF-8", "--format=%P%x1f%an%x1f%ae%x1f%(trailers:only,unfold)", sha).split("\x1f")
    if len(fields) != 4:
        raise CheckError(f"cannot read commit {sha}")
    parents, name, email, trailer_block = fields
    trailers = []
    for line in trailer_block.splitlines():
        key, separator, value = line.partition(":")
        if separator:
            trailers.append((key.strip().lower(), value.strip()))
    return Commit(sha, parents.split(), Identity(name, email), trailers)


def is_clean_merge(repo: Path, commit: Commit) -> bool:
    """True if the commit merges two parents and its content is exactly what Git produces when it merges them without help."""
    if len(commit.parents) != 2:
        return False
    # Exit status 1 means the merge has conflicts, which the merge commit must have resolved by hand. Anything else (for example unrelated histories) is treated the same way: as a merge that needs a sign-off.
    result = git(repo, "merge-tree", "--write-tree", "--no-messages", *commit.parents, ok=range(256))
    if result.returncode != 0:
        return False
    merged_tree = result.stdout.decode("ascii", "replace").split("\n", 1)[0].strip()
    return merged_tree == git_text(repo, "rev-parse", f"{commit.sha}^{{tree}}").strip()


def pull_request_commits(repo: Path, head: str, excluded: Sequence[str]) -> List[str]:
    """The commits that the pull request adds: reachable from its head, but not from the base branch, oldest first."""
    output = git_text(repo, "rev-list", "--reverse", "--topo-order", head, "--not", *excluded)
    return output.split()


# ---------------------------------------------------------------------------
# The rules
# ---------------------------------------------------------------------------


@dataclass
class Verdict:
    commit: Commit
    kind: str  # "person", "agent" or "merge"
    problems: List[str] = field(default_factory=list)
    note: str = ""


def check_commit(commit: Commit, agents: Dict[str, str], clean_merge: bool) -> Verdict:
    if clean_merge:
        return Verdict(commit, "merge", note="a clean merge: it adds nothing of its own, so it needs no sign-off")
    problems = []
    sign_offs = [parse_identity(value) for value in commit.trailer_values(SIGN_OFF)]
    for identity in sign_offs:
        if identity is not None and is_agent(identity, agents):
            problems.append(f'is signed off by the AI agent "{identity}", but only a person can sign off: remove that line')
    if is_agent(commit.author, agents):
        co_authors = [parse_identity(value) for value in commit.trailer_values(CO_AUTHOR)]
        if not any(identity is not None and is_agent(identity, agents) for identity in co_authors):
            problems.append(f'was written by the AI agent "{commit.author}" but does not name it in a "Co-authored-by:" line')
        return Verdict(commit, "agent", problems, "written by an AI agent: the pull request description needs a person's sign-off")
    if is_example_address(commit.author.email):
        problems.append(f'was made under the example address "{commit.author}", which cannot sign off: set your own name and email address with `git config user.name` and `git config user.email`, then make the commit again (`git commit --amend --reset-author --signoff` redoes the last one)')
    elif not any(identity is not None and identity.same_email(commit.author) for identity in sign_offs):
        problems.append(f'has no "Signed-off-by:" line with its author\'s email address, such as "Signed-off-by: {commit.author}"')
    return Verdict(commit, "person", problems, "signed off by its author")


@dataclass
class Description:
    people: List[Identity]  # valid sign-offs by people
    problems: List[str]
    malformed: int  # "Signed-off-by:" lines that are not "Name <email>", such as the template's placeholder
    examples: int  # "Signed-off-by:" lines with an example address, such as "you@example.com", which do not count


def check_description(text: str, agents: Dict[str, str]) -> Description:
    people: List[Identity] = []
    problems: List[str] = []
    malformed = examples = 0
    for line in HTML_COMMENT.sub("", text).splitlines():
        match = DESCRIPTION_SIGN_OFF.fullmatch(line)
        if match is None:
            continue
        identity = parse_identity(match.group("value"))
        if identity is None:
            malformed += 1
        elif is_agent(identity, agents):
            problems.append(f'is signed off by the AI agent "{identity}", but only a person can sign off: remove that line')
        elif is_example_address(identity.email):
            examples += 1
        else:
            people.append(identity)
    return Description(people, problems, malformed, examples)


@dataclass
class Report:
    verdicts: List[Verdict]
    description: Optional[Description]  # None when no description was given (a local run)
    description_problems: List[str]

    @property
    def agent_commits(self) -> int:
        return sum(1 for verdict in self.verdicts if verdict.kind == "agent")

    @property
    def ok(self) -> bool:
        return not self.description_problems and all(not verdict.problems for verdict in self.verdicts)


def check(repo: Path, head: str, excluded: Sequence[str], description_text: Optional[str], agents: Dict[str, str]) -> Report:
    verdicts = []
    for sha in pull_request_commits(repo, head, excluded):
        commit = read_commit(repo, sha)
        verdicts.append(check_commit(commit, agents, is_clean_merge(repo, commit)))
    report = Report(verdicts, None, [])
    if description_text is None:
        return report
    report.description = description = check_description(description_text, agents)
    report.description_problems.extend(description.problems)
    if report.agent_commits and not description.people:
        problem = 'has no "Signed-off-by: Your Name <your email address>" line from the person who submits this pull request, which is needed because AI agents wrote some of its commits'
        if description.examples:
            problem += "; a sign-off with an example address such as you@example.com does not count: write your own name and email address"
        if description.malformed:
            problem += '; a "Signed-off-by:" line must hold a name and an email address in angle brackets (replace the template\'s placeholder with your own)'
        report.description_problems.append(problem)
    return report


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------

def visible(text: str) -> str:
    """Text that may come from a commit or a description, with every control character (Unicode category Cc, such as a carriage return or the escape character that starts terminal colour codes) and every line or paragraph separator written out as an escape such as "\\r". GitHub's runner ends a log line at a carriage return too, so such a character could otherwise start a forged workflow command of its own. Comparisons use the raw text; only output goes through here."""
    return "".join(repr(char)[1:-1] if unicodedata.category(char) in ("Cc", "Zl", "Zp") else char for char in text)


HOW_TO_FIX = [
    "How to fix it (CONTRIBUTING.md, \"Developer Certificate of Origin\"):",
    "  - your own commits: sign them off with `git commit --amend --signoff` (the last commit) or `git rebase --signoff <base branch>` (all of them), then push your branch again;",
    "  - commits written by an AI agent: the person submitting the pull request adds a line \"Signed-off-by: Your Name <your email address>\", with their own name and email address, to its description; editing the description runs this check again.",
]


def describe(report: Report) -> List[str]:
    lines = [f"DCO check of the {len(report.verdicts)} commit(s) in this pull request (rules: CONTRIBUTING.md, \"Developer Certificate of Origin\"):"]
    for verdict in report.verdicts:
        status = "FAILED" if verdict.problems else "ok"
        detail = "; ".join(verdict.problems) if verdict.problems else verdict.note
        lines.append(f"  {status:<6}  {verdict.commit.sha[:12]}  {verdict.kind:<6}  {verdict.commit.author}: {detail}")
    if report.description is None:
        if report.agent_commits:
            lines.append(f"  note    {report.agent_commits} commit(s) were written by an AI agent: the pull request description will need a person's \"Signed-off-by:\" line (not checked here; pass --description FILE to check it).")
    else:
        for problem in report.description_problems:
            lines.append(f"  FAILED  pull request description {problem}")
        if not report.description_problems:
            if report.agent_commits:
                people = ", ".join(str(person) for person in report.description.people)
                lines.append(f"  ok      pull request description: signed off by {people}")
            else:
                lines.append("  ok      pull request description: no sign-off needed, because no commit was written by an AI agent")
    lines.append("")
    if report.ok:
        lines.append("Result: every DCO rule is met.")
    else:
        lines.append("Result: FAILED.")
        lines.extend(HOW_TO_FIX)
    return [visible(line) for line in lines]


def annotation(message: str) -> str:
    """A GitHub Actions error annotation. Control characters are written out visibly and "%" is escaped, so text from a commit can never start a workflow command of its own."""
    return f"::error title=DCO::{visible(message).replace('%', '%25')}"


def annotations(report: Report) -> List[str]:
    found = []
    for verdict in report.verdicts:
        for problem in verdict.problems:
            found.append(annotation(f"Commit {verdict.commit.sha[:12]} {problem}."))
    for problem in report.description_problems:
        found.append(annotation(f"The pull request description {problem}."))
    return found


def _cell(text: str) -> str:
    """Text for a Markdown table cell: no control characters, HTML, Markdown formatting or column breaks."""
    text = html.escape(visible(text), quote=False).replace("|", "&#124;")
    return re.sub(r"([\\`*_\[\]])", r"\\\1", text)


def summary(report: Report) -> str:
    rows = ["## DCO check", "", "| Commit | Written by | Result |", "|---|---|---|"]
    for verdict in report.verdicts:
        result = "; ".join(verdict.problems) if verdict.problems else verdict.note
        icon = "❌" if verdict.problems else "✅"
        rows.append(f"| `{verdict.commit.sha[:12]}` | {_cell(verdict.kind)}: {_cell(str(verdict.commit.author))} | {icon} {_cell(result)} |")
    if report.description is not None:
        if report.description_problems:
            for problem in report.description_problems:
                rows.append(f"| description | | ❌ {_cell('The pull request description ' + problem)} |")
        elif report.agent_commits:
            people = ", ".join(str(person) for person in report.description.people)
            rows.append(f"| description | | ✅ {_cell('signed off by ' + people)} |")
    rows.append("")
    rows.append("Every DCO rule is met." if report.ok else "\n".join(["**Failed.**", ""] + [line.replace("  - ", "- ") for line in HOW_TO_FIX[1:]]))
    return "\n".join(rows) + "\n"


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def from_event(repo: Path, event_file: Path) -> Tuple[str, List[str], str]:
    """Reads the head, the commits to exclude and the description from a pull_request event."""
    try:
        event = json.loads(event_file.read_text(encoding="utf-8"))
        pull_request = event["pull_request"]
        base_sha, base_ref = pull_request["base"]["sha"], pull_request["base"]["ref"]
        head_sha = pull_request["head"]["sha"]
        description = pull_request.get("body") or ""
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise CheckError(f"{event_file} is not a pull_request event ({type(error).__name__}: {error})") from None
    if not isinstance(base_ref, str) or not isinstance(description, str):
        raise CheckError(f"{event_file} is not a pull_request event")
    for object_id in (base_sha, head_sha):
        if not isinstance(object_id, str) or not OBJECT_ID.fullmatch(object_id):
            raise CheckError(f"{event_file}: {object_id!r} is not a commit ID")
    excluded = [resolve(repo, base_sha)]
    # The base branch may have moved on since the event's base commit, and the pull request may have merged it in: its newer commits are on the base branch already, so they are excluded too.
    result = git(repo, "rev-parse", "--verify", "--quiet", "--end-of-options", f"refs/remotes/origin/{base_ref}^{{commit}}", ok=(0, 1))
    if result.returncode == 0:
        excluded.append(result.stdout.decode("ascii", "replace").strip())
    return resolve(repo, head_sha), excluded, description


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Checks the DCO sign-offs of a pull request (CONTRIBUTING.md, \"Developer Certificate of Origin\").")
    parser.add_argument("--event", type=Path, help="the GitHub pull_request event file ($GITHUB_EVENT_PATH)")
    parser.add_argument("--base", help="local run: the branch the pull request goes into, for example origin/main")
    parser.add_argument("--head", help="local run: the last commit of the pull request, for example HEAD")
    parser.add_argument("--description", type=Path, help="local run: a file with the pull request description, to check it too")
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="the Git repository (default: the current folder)")
    parser.add_argument("--agents", type=Path, default=AGENTS_FILE, help="the list of AI agent identities (default: agents.txt next to this script)")
    args = parser.parse_args(argv)
    if (args.event is None) == (args.base is None) or (args.base is None) != (args.head is None) or (args.event and args.description):
        parser.error("use either --event FILE, or --base REV --head REV [--description FILE]")
    in_actions = os.environ.get("GITHUB_ACTIONS") == "true"
    try:
        agents = load_agents(args.agents)
        require_git(args.repo)
        if args.event is not None:
            head, excluded, description = from_event(args.repo, args.event)
        else:
            head, excluded = resolve(args.repo, args.head), [resolve(args.repo, args.base)]
            description = None
            if args.description is not None:
                try:
                    description = args.description.read_text(encoding="utf-8")
                except OSError as error:
                    raise CheckError(f"cannot read {args.description}: {error.strerror}") from None
        report = check(args.repo, head, excluded, description, agents)
    except CheckError as error:
        print(visible(f"The DCO check could not run: {error}"), file=sys.stderr)
        if in_actions:
            print(annotation(f"The DCO check could not run: {error}"))
        return 2
    print("\n".join(describe(report)))
    if in_actions:
        for line in annotations(report):
            print(line)
        summary_file = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary_file:
            with open(summary_file, "a", encoding="utf-8") as output:
                output.write(summary(report))
    return 0 if report.ok else 1


if __name__ == "__main__":
    sys.exit(main())
