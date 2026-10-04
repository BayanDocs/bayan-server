# Integration kits (Apache-2.0)

This folder will hold everything companies and developers need to integrate with a BayanDocs server: machine-readable API descriptions, SDKs and examples. It is empty until the first kit arrives with work package SRV-105.

**License:** unlike the rest of this repository (AGPL-3.0-or-later), everything in this folder is licensed under the Apache License 2.0 (SPDX: `Apache-2.0`); the full text is in [LICENSE](LICENSE). You may use these kits in software under any license, including proprietary software, as long as you keep their license notices. See the [licensing FAQ](https://github.com/BayanDocs/docs/blob/HEAD/LICENSING.md) and [ADR-0003](https://github.com/BayanDocs/docs/blob/HEAD/adr/0003-licensing-and-contribution-model.md).

**Rules for this folder:**

- Kits are written against the published protocol specifications in [docs/specs/protocols/](https://github.com/BayanDocs/docs/tree/HEAD/specs/protocols). They never contain, copy from or depend on the AGPL server code or the GPL engine code.
- Every kit that can be distributed on its own (for example a package published to a registry) carries its own copy of the Apache-2.0 license text.
