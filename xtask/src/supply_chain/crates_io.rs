//! When a crate version was published on crates.io.
//!
//! The answer comes from the crates.io sparse index (`https://index.crates.io/`), whose line for each version carries the publish time as `pubtime`; that is the source Cargo itself reads, served by a content delivery network, so it is cheap to ask. If a line has no `pubtime`, the crates.io API (`https://crates.io/api/v1/crates/<name>/<version>`) answers with the version's `created_at`; the API allows at most one request per second from tools like this one (<https://crates.io/data-access>), so its requests are spaced out. Every request names this tool and its repository in a descriptive User-Agent, as crates.io asks, and each crate's index file is fetched once per run.
//!
//! The requests are made by `curl`, which every supported platform has (Windows 10 and later include it), so that xtask needs no HTTP or TLS library of its own.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{json, time};

/// The longest crate name crates.io accepts.
const MAX_NAME_LENGTH: usize = 64;

/// How long one request may take, in seconds, including retries.
const TIMEOUT_SECONDS: &str = "60";

/// Fetches the body of an HTTPS address.
pub trait Fetch {
    /// The body of a successful answer to a GET request for `url`.
    fn get(&mut self, url: &str) -> Result<String, String>;
}

/// [`Fetch`] with `curl`.
pub struct Curl {
    user_agent: String,
}

impl Curl {
    /// A fetcher that sends `user_agent` with every request.
    pub fn new(user_agent: String) -> Self {
        Self { user_agent }
    }
}

impl Fetch for Curl {
    fn get(&mut self, url: &str) -> Result<String, String> {
        let output = Command::new("curl")
            // `--disable` must come first: it stops curl from reading a personal configuration file (.curlrc) that could change what it does.
            .args(["--disable", "--silent", "--show-error", "--fail", "--proto", "=https", "--tlsv1.2"])
            // Retries back off, and follow the server's Retry-After when it asks to slow down (HTTP 429). Redirects are not followed.
            .args(["--retry", "3", "--max-time", TIMEOUT_SECONDS, "--user-agent", &self.user_agent])
            .arg(url)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| {
                format!("could not run curl, which reads the crates.io index: {error}. Install curl, or run the check where it is available.")
            })?;
        if !output.status.success() {
            return Err(format!(
                "could not fetch {url}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| format!("{url} answered with text that is not UTF-8"))
    }
}

/// Publish times of crate versions, looked up on crates.io.
pub struct CratesIo<'a> {
    fetch: &'a mut dyn Fetch,
    /// The versions of every crate looked up so far, with the publish time that the index records (if any).
    index: BTreeMap<String, Vec<(String, Option<i64>)>>,
    /// The least time between two requests to the API.
    api_interval: Duration,
    last_api_request: Option<Instant>,
}

impl<'a> CratesIo<'a> {
    /// Looks versions up through `fetch`, leaving at least `api_interval` between two requests to the API.
    pub fn new(fetch: &'a mut dyn Fetch, api_interval: Duration) -> Self {
        Self {
            fetch,
            index: BTreeMap::new(),
            api_interval,
            last_api_request: None,
        }
    }

    /// When version `version` of the crate `name` was published, in seconds since 1970.
    pub fn published(&mut self, name: &str, version: &str) -> Result<i64, String> {
        check_name(name)?;
        check_version(version)?;
        let key = name.to_ascii_lowercase();
        if !self.index.contains_key(&key) {
            let url = format!("https://index.crates.io/{}", index_path(&key));
            let body = self.fetch.get(&url)?;
            let versions =
                read_index(&key, &body).map_err(|problem| format!("{url}: {problem}"))?;
            self.index.insert(key.clone(), versions);
        }
        let found = self.index.get(&key).and_then(|versions| {
            versions
                .iter()
                .find(|(listed, _)| listed == version)
                .map(|(_, published)| *published)
        });
        match found {
            Some(Some(published)) => Ok(published),
            Some(None) => self.created_at(name, version),
            None => Err(format!(
                "{name} {version} is not in the crates.io index, which lists every version ever published (yanked ones too)"
            )),
        }
    }

    /// The publish time of a version according to the crates.io API, for an index line without `pubtime`.
    fn created_at(&mut self, name: &str, version: &str) -> Result<i64, String> {
        if let Some(last) = self.last_api_request {
            let elapsed = last.elapsed();
            if elapsed < self.api_interval {
                std::thread::sleep(self.api_interval - elapsed);
            }
        }
        let url = format!("https://crates.io/api/v1/crates/{name}/{version}");
        let body = self.fetch.get(&url);
        self.last_api_request = Some(Instant::now());
        let answer = json::parse(&body?).map_err(|problem| format!("{url}: {problem}"))?;
        let created_at = answer
            .get("version")
            .and_then(|version| version.string("created_at"))
            .ok_or_else(|| format!("{url}: the answer has no `version.created_at`"))?;
        time::parse(created_at).map_err(|problem| format!("{url}: {problem}"))
    }
}

/// Crate names on crates.io are ASCII letters, digits, `-` and `_`. Anything else is refused before it becomes part of an address.
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > MAX_NAME_LENGTH
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!("`{name}` is not a crates.io crate name"));
    }
    Ok(())
}

/// Versions are ASCII letters, digits, `.`, `-` and `+` (semantic versioning). Anything else is refused before it becomes part of an address.
fn check_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || version.len() > 128
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        return Err(format!("`{version}` is not a crate version"));
    }
    Ok(())
}

/// The path of a crate's file in the sparse index, from its lowercase name: `1/a`, `2/ab`, `3/a/abc`, `hy/pe/hyper`.
fn index_path(name: &str) -> String {
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

/// The versions in a crate's index file, one JSON object per line, each with its `pubtime` if the line has one.
fn read_index(name: &str, body: &str) -> Result<Vec<(String, Option<i64>)>, String> {
    let mut versions = Vec::new();
    for (index, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry =
            json::parse(line).map_err(|problem| format!("line {}: {problem}", index + 1))?;
        let listed_name = entry
            .string("name")
            .ok_or_else(|| format!("line {} has no name", index + 1))?;
        if !listed_name.eq_ignore_ascii_case(name) {
            return Err(format!(
                "line {} describes the crate `{listed_name}`, not `{name}`",
                index + 1
            ));
        }
        let version = entry
            .string("vers")
            .ok_or_else(|| format!("line {} has no version", index + 1))?;
        let published = entry
            .string("pubtime")
            .map(time::parse)
            .transpose()
            .map_err(|problem| format!("line {}: {problem}", index + 1))?;
        versions.push((version.to_owned(), published));
    }
    if versions.is_empty() {
        return Err("the index file lists no versions".to_owned());
    }
    Ok(versions)
}

#[cfg(test)]
pub mod testing {
    //! A [`Fetch`] that replays answers recorded from crates.io, so the tests never use the network.

    use std::collections::BTreeMap;

    use super::Fetch;

    /// Answers each address with a recorded body, and remembers what was asked.
    pub struct Recorded {
        answers: BTreeMap<&'static str, &'static str>,
        /// The addresses asked for, in order.
        pub requests: Vec<String>,
    }

    impl Recorded {
        /// The recordings in `fixtures/`, see `fixtures/README.md`.
        pub fn crates_io() -> Self {
            Self::with(&[
                (
                    "https://index.crates.io/hy/pe/hyper",
                    include_str!("fixtures/index.crates.io/hy/pe/hyper"),
                ),
                (
                    "https://index.crates.io/ze/ro/zerocopy",
                    include_str!("fixtures/index.crates.io/ze/ro/zerocopy"),
                ),
                (
                    "https://index.crates.io/se/rd/serde",
                    include_str!("fixtures/index.crates.io/se/rd/serde"),
                ),
                (
                    "https://crates.io/api/v1/crates/serde/1.0.228",
                    include_str!("fixtures/crates.io/api/v1/crates/serde/1.0.228"),
                ),
            ])
        }

        /// Answers made of `answers`, pairs of an address and its body.
        pub fn with(answers: &[(&'static str, &'static str)]) -> Self {
            Self {
                answers: answers.iter().copied().collect(),
                requests: Vec::new(),
            }
        }
    }

    impl Fetch for Recorded {
        fn get(&mut self, url: &str) -> Result<String, String> {
            self.requests.push(url.to_owned());
            self.answers
                .get(url)
                .map(|body| (*body).to_owned())
                .ok_or_else(|| {
                    format!("could not fetch {url}: The requested URL returned error: 404")
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Recorded;
    use super::*;

    #[test]
    fn finds_the_index_file_of_any_name() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("hyper"), "hy/pe/hyper");
        assert_eq!(index_path("serde_json"), "se/rd/serde_json");
    }

    #[test]
    fn reads_publish_times_from_the_index_and_fetches_each_crate_once() {
        let mut recorded = Recorded::crates_io();
        let mut crates_io = CratesIo::new(&mut recorded, Duration::ZERO);
        assert_eq!(
            crates_io.published("hyper", "1.12.0"),
            time::parse("2026-10-06T15:57:18Z")
        );
        assert_eq!(
            crates_io.published("Hyper", "1.11.1"),
            time::parse("2026-08-28T12:22:30Z")
        );
        assert_eq!(
            crates_io.published("zerocopy", "0.8.60"),
            time::parse("2026-10-05T23:02:10Z")
        );
        assert_eq!(
            recorded.requests,
            [
                "https://index.crates.io/hy/pe/hyper",
                "https://index.crates.io/ze/ro/zerocopy"
            ]
        );
    }

    #[test]
    fn asks_the_api_when_the_index_has_no_publish_time() {
        let mut recorded = Recorded::crates_io();
        let mut crates_io = CratesIo::new(&mut recorded, Duration::ZERO);
        // The recorded line of serde 1.0.228 has its pubtime removed; the API's `version.created_at` takes its place, not the `created_at` of the publisher's account that the answer also contains.
        assert_eq!(
            crates_io.published("serde", "1.0.228"),
            time::parse("2025-09-27T16:51:35Z")
        );
        assert_eq!(
            crates_io.published("serde", "1.0.229"),
            time::parse("2026-07-18T23:05:13Z")
        );
        assert_eq!(
            recorded.requests,
            [
                "https://index.crates.io/se/rd/serde",
                "https://crates.io/api/v1/crates/serde/1.0.228"
            ]
        );
    }

    #[test]
    fn spaces_out_requests_to_the_api() {
        let mut recorded = Recorded::with(&[
            (
                "https://index.crates.io/1/a",
                "{\"name\":\"a\",\"vers\":\"1.0.0\"}\n{\"name\":\"a\",\"vers\":\"2.0.0\"}\n",
            ),
            (
                "https://crates.io/api/v1/crates/a/1.0.0",
                "{\"version\":{\"created_at\":\"2020-01-01T00:00:00Z\"}}",
            ),
            (
                "https://crates.io/api/v1/crates/a/2.0.0",
                "{\"version\":{\"created_at\":\"2021-01-01T00:00:00Z\"}}",
            ),
        ]);
        let mut crates_io = CratesIo::new(&mut recorded, Duration::from_millis(200));
        let started = Instant::now();
        assert_eq!(
            crates_io.published("a", "1.0.0"),
            time::parse("2020-01-01T00:00:00Z")
        );
        assert_eq!(
            crates_io.published("a", "2.0.0"),
            time::parse("2021-01-01T00:00:00Z")
        );
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn reports_what_crates_io_does_not_know() {
        let mut recorded = Recorded::crates_io();
        let mut crates_io = CratesIo::new(&mut recorded, Duration::ZERO);
        let missing = crates_io.published("hyper", "9.9.9").unwrap_err();
        assert!(
            missing.contains("hyper 9.9.9 is not in the crates.io index"),
            "{missing}"
        );
        let unknown = crates_io.published("no-such-crate", "1.0.0").unwrap_err();
        assert!(unknown.contains("404"), "{unknown}");
    }

    #[test]
    fn refuses_names_and_versions_that_are_not_safe_in_an_address() {
        let mut recorded = Recorded::with(&[]);
        let mut crates_io = CratesIo::new(&mut recorded, Duration::ZERO);
        for (name, version) in [
            ("../api", "1.0.0"),
            ("a b", "1.0.0"),
            ("", "1.0.0"),
            ("a".repeat(65).as_str(), "1.0.0"),
            ("hyper", "1.0.0/../../x"),
            ("hyper", "1.0.0?x"),
            ("hyper", ""),
        ] {
            assert!(
                crates_io.published(name, version).is_err(),
                "{name} {version}"
            );
        }
        assert!(recorded.requests.is_empty());
    }

    #[test]
    fn rejects_index_files_it_cannot_trust() {
        assert!(read_index("hyper", "").is_err());
        assert!(read_index("hyper", "<html>not json</html>").is_err());
        assert!(read_index("hyper", "{\"name\":\"other\",\"vers\":\"1.0.0\"}").is_err());
        assert!(read_index("hyper", "{\"name\":\"hyper\"}").is_err());
        assert!(
            read_index(
                "hyper",
                "{\"name\":\"hyper\",\"vers\":\"1.0.0\",\"pubtime\":\"yesterday\"}"
            )
            .is_err()
        );
    }
}
