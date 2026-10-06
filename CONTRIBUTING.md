# Contributing to BayanDocs

Thank you for helping to build BayanDocs, a free and open-source word processor that opens, edits and saves Microsoft Word documents faithfully. This is the **bayan-server** repository: the collaboration server, a single Rust program that relays end-to-end encrypted changes and never sees document content. This guide explains how to contribute to it. The same rules apply in all five BayanDocs repositories, and to everyone who contributes, people and AI agents alike.

## Before you start

- **Read the rules.** The [Agent Operating Manual (AGENTS.md)](https://github.com/BayanDocs/docs/blob/main/AGENTS.md) is the canonical set of rules for every contributor, human or AI. [How work gets handed off, done and reviewed](https://github.com/BayanDocs/docs/blob/main/plan/06-agent-workflow.md) describes the workflow, and [LICENSING.md](https://github.com/BayanDocs/docs/blob/main/LICENSING.md) explains the licenses in plain language.
- **Decisions are recorded.** Accepted [architecture decision records (ADRs)](https://github.com/BayanDocs/docs/blob/main/adr/README.md) are binding. To change a decision, propose a new ADR in a pull request to the docs repository instead of writing code that departs from it.
- **Be kind.** Everyone who takes part follows the [Code of Conduct](CODE_OF_CONDUCT.md).
- **Security problems are reported privately**, never in a public issue: see [SECURITY.md](SECURITY.md).

## Proposing a change

1. **Small fixes** (a typo, a broken link, an obvious bug) can go straight to a pull request.
2. **Larger changes start with an issue** (bug report or feature request), so they can be planned. Planned work is described in [work packages](https://github.com/BayanDocs/docs/blob/main/workpackages/README.md): each has a brief, and one work package becomes one pull request.
3. **Work on a branch** of your fork, or of the repository if you have access. Keep commits small and logical, and stay within what the issue or work package asks for; note anything else you find as a follow-up instead of changing it.
4. **Run the verification gate before every push.** In this repository it is `cargo xtask verify` (install the pinned tools first with `scripts/dev-setup.sh`). Never weaken, skip or delete a test or check to make a change pass. Every behavior change comes with tests, and every bug fix with a test that would have caught it.
5. **Open a pull request against `main`** and fill in the template, which is the [hand-off template](https://github.com/BayanDocs/docs/blob/main/plan/06-agent-workflow.md#hand-off-template): what changed and why, evidence for each acceptance criterion, the commands you ran, dependencies, deviations, follow-ups and open questions. Explain your change in plain language: not every reviewer is an expert in every language the project uses.
6. **Every pull request is squash-merged**: it becomes a single commit on `main`, titled with the pull request's title. Write that title as a Conventional Commit (below).

[AGENTS.md](AGENTS.md) has this repository's own rules and explains how to install its tools; the canonical rules are in the [docs repository's AGENTS.md](https://github.com/BayanDocs/docs/blob/main/AGENTS.md).

## Commit messages and pull request titles

BayanDocs uses [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): summary`, where the scope is optional. Common types are `feat` (a new feature), `fix` (a bug fix), `docs`, `test`, `refactor`, `perf`, `build`, `ci` and `chore`; dependency updates are `chore(deps): …`. Examples: `feat(layout): break lines at soft hyphens`, `fix(opc): reject ZIP entries with absolute paths`, `docs: explain the DCO check`. A breaking change adds `!` after the type, as in `feat(protocol)!: …`.

## Developer Certificate of Origin

BayanDocs uses the [Developer Certificate of Origin (DCO)](https://developercertificate.org/) instead of a contributor license agreement. By signing off a contribution you certify, in short, that you wrote it or otherwise have the right to submit it under the license of the files it changes, and that you understand that the contribution and your sign-off (your name and email address) are public and kept permanently. The full text is short; read it once at the link above.

### Signing off your commits

Add `--signoff` (or `-s`) when you commit:

```sh
git commit --signoff -m "fix(opc): reject ZIP entries with absolute paths"
```

Git then ends the message with a line such as `Signed-off-by: Jane Doe <jane@example.com>`, taken from your Git settings (`git config user.name` and `git config user.email`). The sign-off must carry the same email address as the commit's author, letter case aside; the name in it may be written differently. Use your own email address: an address at a domain reserved for examples, such as the `jane@example.com` above, never counts as a sign-off.

If you forgot, sign off the last commit with `git commit --amend --signoff --no-edit`, or every commit of your branch with `git rebase --signoff origin/main`, and then update your own branch with `git push --force-with-lease`. Never force-push a branch that someone else is working on.

A commit made in GitHub's web editor has no sign-off unless you add one, and its author email is the one GitHub chooses (often your `…@users.noreply.github.com` address), so it is simplest to make changes with Git on your own computer.

### AI-assisted contributions

The DCO is a certification that only a person can make ([ADR-0003 §5](https://github.com/BayanDocs/docs/blob/main/adr/0003-licensing-and-contribution-model.md#5-contributions)):

- **AI agents never sign off**, neither in a commit nor in a pull request description.
- **A commit written by an AI agent** has the agent as its author, using an identity listed in `.github/dco/agents.txt` (for example `Claude <noreply@anthropic.com>`), and names the agent in a `Co-authored-by:` line. Claude Code adds such a line, and a `Claude-Session:` link, by itself.
- **The person who submits the pull request certifies the agent's work** (for now, the project owner). Before merging, they replace the placeholder in the template's "Developer Certificate of Origin" section with their own line, `Signed-off-by: Your Name <your email address>`, on a line of its own, and keep the same line at the end of the squash-merge commit's message.
- **If an agent commits under your own name** (for example Claude Code running on your computer with your Git settings), those commits are yours: review them, then sign them off yourself, for example with `git rebase --signoff origin/main`, before you push.

### What the DCO check checks

The DCO workflow (`.github/workflows/dco.yml`) checks every commit that a pull request adds. Commits that are already on `main` are not checked. It runs again whenever the pull request's description is edited, so adding a sign-off there is all it takes.

| Commit | What it needs |
|---|---|
| Written by a person | A `Signed-off-by:` line with its author's email address (the name may be written differently). |
| Written by an AI agent (its author is listed in `.github/dco/agents.txt`) | A `Co-authored-by:` line naming the agent, and no sign-off by the agent. The pull request description must then contain a person's `Signed-off-by:` line. |
| With co-authors (`Co-authored-by:` lines) | The author's sign-off covers the whole commit, including the parts that co-authors wrote; co-authors may add their own sign-off. An AI agent named as a co-author is credited, never signing off. |
| A merge commit that Git could have made by itself, such as the one GitHub's "Update branch" button creates | Nothing: it adds no content of its own. |
| A merge commit that resolves a conflict or changes anything | The same as any other commit, by its author. |
| Any commit, and the description | No `Signed-off-by:` line may name an AI agent, and none counts if its email address is at a domain reserved for examples (`example.com`, `example.net`, `example.org`, or one ending in `.example`): those are placeholders, not anyone's address. A commit whose author has such an address fails. |

A sign-off counts only where `git commit --signoff` puts it: in the block of lines at the very end of the commit message. In a pull request description it must be a line of its own, not hidden in an HTML comment (`<!-- … -->`). To check your branch before pushing, run `python3 .github/dco/check_dco.py --base origin/main --head HEAD`, and add `--description FILE` to check a description saved in a file as well.

The check and the list of AI agents are in `.github/dco/`, identical in all five repositories. Change them in a pull request to all five together.

## Licensing of contributions

BayanDocs is free software, and contributions keep it that way ([LICENSING.md](https://github.com/BayanDocs/docs/blob/main/LICENSING.md), [ADR-0003](https://github.com/BayanDocs/docs/blob/main/adr/0003-licensing-and-contribution-model.md)):

- **Inbound = outbound.** When you change a file, your contribution is licensed under that file's license: the one in the file's own SPDX header, or else the one `REUSE.toml` gives it. You keep your copyright.
- **No contributor license agreement.** Nobody, including the project itself or a future owner of it, can relicense your work.

### In this repository

| Files | License (SPDX identifier) |
|---|---|
| Everything not listed below | `AGPL-3.0-or-later` |
| `integrations/`: the integration kits (API descriptions, SDKs, examples) | `Apache-2.0` |
| `CODE_OF_CONDUCT.md`: the Contributor Covenant, by its authors | `CC-BY-4.0` |

**The Apache-2.0 area (`integrations/`)** exists so that anyone can build BayanDocs clients and integrations under any license. Code and text there must never contain, copy from or depend on GPL or AGPL code, including the rest of BayanDocs: write it against the published specifications ([ADR-0003 §2](https://github.com/BayanDocs/docs/blob/main/adr/0003-licensing-and-contribution-model.md#2-rules-for-the-apache-20-areas)). The GPL and AGPL parts may depend on it.

### Rules for every repository

- **Every file states its license.** Either the file has its own SPDX header, or `REUSE.toml` declares a license for it. `reuse lint` must pass ([REUSE](https://reuse.software)); CI runs it on every pull request. Add a new license text only with `reuse download <SPDX-ID>`.
- **Material copied into users' documents** (templates, sample content, default styles) is `CC0-1.0`, and **fonts made by the project** are `OFL-1.1`. Whoever creates the first such folder declares it in `REUSE.toml` and adds the license text with `reuse download`.
- **Third-party code** keeps its own license and copyright: record them in `REUSE.toml` or in a `.license` file next to it. Bring in only code whose license is on the allowlist of [ADR-0017](https://github.com/BayanDocs/docs/blob/main/adr/0017-supply-chain-and-dependency-policy.md).
- **Clean room.** Learn how Microsoft Word behaves only from its public specifications (ECMA-376, ISO/IEC 29500, the [MS-*] open specifications) and by observing what it does. Never decompile, disassemble or debug Microsoft software, and never copy code whose license is incompatible with the files you change.

## Dependencies

Every dependency is a risk to the people who use BayanDocs, so the rules are strict ([ADR-0017](https://github.com/BayanDocs/docs/blob/main/adr/0017-supply-chain-and-dependency-policy.md); [AGENTS.md §5](https://github.com/BayanDocs/docs/blob/main/AGENTS.md#5-dependencies-strict--see-adr-0017)):

- **No automated update bots**: no Dependabot version updates, no Renovate, nothing similar. Dependabot security alerts stay on; they open no pull requests.
- **Every version must be at least 24 hours old** when it is installed or pinned. Check its publish date and state it in the pull request.
- **Exact versions and committed lockfiles** (`Cargo.lock`, `pnpm-lock.yaml`); CI installs with `--locked` or `--frozen-lockfile`.
- **Install scripts stay disabled.** A package's install script may run only with an approved justification.
- **Audits and license checks stay green**, and every license is on the allowlist.
- **Every new dependency is justified** in its pull request: what it does, why we cannot reasonably write it, the alternatives considered, its license, how well it is maintained, the publish date of the pinned version, and how many other packages it brings in.
- **Upgrades happen only in the batched dependency session** ([monthly, or right after a security alert](https://github.com/BayanDocs/docs/blob/main/plan/06-agent-workflow.md#monthly-dependency-update-session)), never as a side effect of other work.

## Security and privacy

- Report vulnerabilities privately, as [SECURITY.md](SECURITY.md) explains.
- Treat every document, image, font and network message as hostile input.
- Never commit secrets, tokens, private keys or real personal documents. Test documents are made for the purpose, or have a recorded license.
- Never log, print or report document content, file names or user identifiers.
