# Recorded crates.io answers

Test fixtures for `crates_io.rs` and `lockfile_age.rs`, so that the tests of `check-lockfile-age` never use the network (work package X-003, acceptance criterion AC-2). Each file holds the answer to the address its path names: `index.crates.io/hy/pe/hyper` answers `https://index.crates.io/hy/pe/hyper`, the crates.io sparse index file of the crate `hyper`, and `crates.io/api/v1/crates/serde/1.0.228` answers the crates.io API request of that name.

The values were recorded from index.crates.io and the crates.io API on 2026-10-07 at 02:38 UTC: the versions, checksums and publish times (`pubtime`, and the API's `created_at`) are the real ones. The files keep only the versions the tests use, and of each line only some of its dependencies and features. Two changes are deliberate:

- The line of serde 1.0.228 has no `pubtime`, so that the tests exercise the fallback to the crates.io API, whose recorded answer gives the version's `created_at`. That answer also keeps a second, misleading `created_at`, that of the publisher's account, which the reader must not take.
- hyper 1.12.0 was published on 2026-10-06 at 15:57 UTC, less than 24 hours before the recording, so it is the "too new" version of the tests; zerocopy 0.8.60, published on 2026-10-05 at 23:02 UTC, is just over a day old at the recording time.

When the format of the index or the API changes, record new answers (`curl -A "<a descriptive User-Agent>" <address>`) and keep the test times consistent with the new publish times.
