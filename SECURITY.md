# Security policy

BayanDocs opens documents from untrusted sources, and its collaboration server holds only end-to-end encrypted data, so security problems matter to everyone who uses it. Thank you for helping to keep them safe.

## Reporting a vulnerability

**Please do not report security vulnerabilities in public issues, pull requests or discussions.**

Report them privately through GitHub: open the [**Security** tab of this repository](https://github.com/BayanDocs/bayan-server/security) and choose **Report a vulnerability**. Only you and the maintainers can see the report. If the problem affects another BayanDocs repository, or you are not sure which one, report it here anyway, and we will move it to the right place.

If you cannot use GitHub, email **<security@bayandocs.org>**.

Please include, as far as you can:

- the affected part of BayanDocs, and the version or commit;
- what an attacker could do with the problem;
- the steps to reproduce it, or a proof of concept;
- how we can reach you with questions.

If a document is needed to show the problem, attach a small one made for the purpose: never a real document with personal or confidential content.

## What happens next

- We acknowledge your report within **3 days**.
- We investigate, keep you informed, and agree with you on when the problem becomes public. Please keep it private until a fix is released or that date has come.
- We aim to fix **critical** problems within **7 days** and **high-severity** problems within **30 days** of confirming them. If a fix depends on a new version of a dependency that is less than 24 hours old, we protect users another way in the meantime and update once the version is eligible ([ADR-0017](https://github.com/BayanDocs/docs/blob/main/adr/0017-supply-chain-and-dependency-policy.md)).
- We publish a security advisory through GitHub, request a CVE identifier where appropriate, and credit you unless you prefer to stay anonymous.

These targets come from the project's [quality, security and testing plan](https://github.com/BayanDocs/docs/blob/main/plan/07-quality-security-testing.md).

## Supported versions

BayanDocs has not been released yet. Until version 1.0, only the latest release receives security fixes; before the first release, fixes go to the `main` branch.

| Version | Supported |
|---|---|
| Latest release (or `main`, before the first release) | Yes |
| Any older release | No |

## What we especially want to hear about

- a document that makes BayanDocs run code, fetch a resource from the network without asking, or read files it should not;
- a document or message that crashes the engine or makes it hang or use unbounded memory;
- anything that lets the collaboration server, or anyone on the network, read document content or keys;
- weaknesses in how BayanDocs is built and released, such as its CI, dependencies or signing.

The [threat model](https://github.com/BayanDocs/docs/blob/main/specs/threat-model.md) describes what BayanDocs protects and from whom.
