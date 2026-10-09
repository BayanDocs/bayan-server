#!/usr/bin/env bash
# Installs the tools bayan-server's checks need that are not part of the Rust toolchain, at exact versions verified against SHA-256 hashes (ADR-0017). Idempotent: a tool already installed at the pinned version is left alone. CI runs this script too, so local runs and CI use the same versions.
#
# Usage: scripts/dev-setup.sh [tool ...]   installs the named tools (default: all)
#
# cargo-deny and grype go into $BAYAN_TOOLS_BIN (default ~/.local/bin). The tools that run the MLS spike's tests in WebAssembly (spikes/mls) go into their own folders under $BAYAN_TOOLS_DIR (default ~/.local/share/bayandocs/server-tools), not on PATH, so they never replace a Node.js, browser or wasm-bindgen you use elsewhere; `cargo xtask mls-spike-wasm` finds them there.
#
# Pins change only in the monthly dependency session; keep cargo-deny, grype and Node.js equal to docs/scripts/cloud-environment-setup.sh, Node.js and the Chromium headless shell equal to bayan-web's scripts/dev-setup.sh, and wasm-bindgen equal to the wasm-bindgen crate in spikes/mls/Cargo.lock (the test runner refuses other versions). Every version was at least 24 hours old when pinned.
#   cargo-deny 0.20.2               released 2026-07-09  dependency policy checks in `cargo xtask verify`
#   grype 0.120.0                   released 2026-10-02  container image vulnerability scanner (CI)
#   wasm-bindgen 0.2.129            released 2026-09-25  wasm-bindgen and wasm-bindgen-test-runner, for the MLS spike's WebAssembly tests and size measurement
#   node 24.21.0                    released 2026-09-07  Node.js (only its `node` program), runs the WebAssembly tests in Node.js; hash from nodejs.org's signed SHASUMS256.txt, as in bayan-web
#   chromedriver 153.0.8010.12      released 2026-08-25  drives the browser for the WebAssembly tests in Chromium
#   chrome-headless-shell 153.0.8010.12  released 2026-08-25  Chromium's headless shell (Chrome for Testing), the same build bayan-web tests with
# The two Chrome for Testing archives were hashed from Google's storage.googleapis.com and confirmed identical on npmmirror (cdn.npmmirror.com/binaries/chrome-for-testing/…), a mirror run by a different organization, on 2026-10-07.
# Also needed, from the system: curl, sha256sum, tar with xz support, unzip; python3 and psql for `cargo xtask sqlx-prepare`; Docker for the container build and smoke test; and for the headless shell the libraries Chromium needs (present on GitHub's Ubuntu runners; on a bare Ubuntu install, bayan-web's scripts/dev-setup.sh lists them).
set -euo pipefail

bin_dir="${BAYAN_TOOLS_BIN:-$HOME/.local/bin}"
tools_dir="${BAYAN_TOOLS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/bayandocs/server-tools}"

# name | version | URL | SHA-256 of the archive | what to install | command (relative to the install location) that prints the version
# What to install: "bin:<path>" copies that file from the archive into $bin_dir; "files:<path>,<path>,..." copies those files into $tools_dir/<name>/; "tree:<directory>" installs that directory of the archive as $tools_dir/<name>/.
tools=(
  "cargo-deny|0.20.2|https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz|9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f|bin:cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny|cargo-deny --version"
  "grype|0.120.0|https://github.com/anchore/grype/releases/download/v0.120.0/grype_0.120.0_linux_amd64.tar.gz|a5a1218dce63acdac152a6b3b5bb366e7267e36f4069848cf455543b3fa5700e|bin:grype|grype --version"
  "wasm-bindgen|0.2.129|https://github.com/wasm-bindgen/wasm-bindgen/releases/download/0.2.129/wasm-bindgen-0.2.129-x86_64-unknown-linux-musl.tar.gz|82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e|files:wasm-bindgen-0.2.129-x86_64-unknown-linux-musl/wasm-bindgen,wasm-bindgen-0.2.129-x86_64-unknown-linux-musl/wasm-bindgen-test-runner|wasm-bindgen --version"
  "node|24.21.0|https://nodejs.org/dist/v24.21.0/node-v24.21.0-linux-x64.tar.xz|fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6|files:node-v24.21.0-linux-x64/bin/node|node --version"
  "chromedriver|153.0.8010.12|https://storage.googleapis.com/chrome-for-testing-public/153.0.8010.12/linux64/chromedriver-linux64.zip|b7d5f7c120f7827f3538b416e08b82418eb702f18b680c183f0411c8d7f2df69|files:chromedriver-linux64/chromedriver|chromedriver --version"
  "chrome-headless-shell|153.0.8010.12|https://storage.googleapis.com/chrome-for-testing-public/153.0.8010.12/linux64/chrome-headless-shell-linux64.zip|a9da028861a0cf789ff25c2fed45f5f1aaf969ed9247835b6a7821a4f7af9d1d|tree:chrome-headless-shell-linux64|chrome-headless-shell --version"
)

if [ "$(uname -s)-$(uname -m)" != "Linux-x86_64" ]; then
  echo "dev-setup: only Linux x86-64 is supported; install the versions listed in this script by other means" >&2
  exit 1
fi

wanted=("$@")
for name in "${wanted[@]}"; do
  printf '%s\n' "${tools[@]}" | grep -q "^$name|" || { echo "dev-setup: unknown tool $name" >&2; exit 1; }
done

# Prints whether `command` (a program and its arguments) prints `version` as a separate word, as in "cargo-deny 0.20.2", "v24.21.0" or "ChromeDriver 153.0.8010.12 (…)".
reports_version() {
  local version="$1"
  shift
  "$@" 2>/dev/null | grep -qE "(^| |v)${version//./\\.}( |$)"
}

mkdir -p "$bin_dir" "$tools_dir"
for entry in "${tools[@]}"; do
  IFS='|' read -r name version url sha256 install version_command <<<"$entry"
  if [ "${#wanted[@]}" -gt 0 ] && ! printf '%s\n' "${wanted[@]}" | grep -qx "$name"; then
    continue
  fi
  kind="${install%%:*}"
  paths="${install#*:}"
  read -r -a version_args <<<"$version_command"
  if [ "$kind" = bin ]; then
    # Already installed at the pinned version (here or elsewhere on PATH)?
    for candidate in "$bin_dir/$name" "$(command -v "$name" || true)"; do
      if [ -n "$candidate" ] && [ -x "$candidate" ] && reports_version "$version" "$candidate" "${version_args[@]:1}"; then
        echo "dev-setup: $name $version already installed ($candidate)"
        continue 2
      fi
    done
  else
    # A folder of its own, marked with the hash of the archive it came from once it is complete.
    target="$tools_dir/$name"
    if [ "$(cat "$target/.bayandocs-sha256" 2>/dev/null)" = "$sha256" ] && reports_version "$version" "$target/${version_args[0]}" "${version_args[@]:1}"; then
      echo "dev-setup: $name $version already installed ($target)"
      continue
    fi
  fi
  tmp="$(mktemp -d)"
  trap 'rm -rf -- "$tmp"' EXIT
  echo "dev-setup: installing $name $version"
  archive="$tmp/$(basename "$url")"
  curl --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 --output "$archive" "$url"
  echo "$sha256  $archive" | sha256sum --check --quiet - || { echo "dev-setup: checksum mismatch for $name" >&2; exit 1; }
  IFS=',' read -r -a members <<<"$paths"
  # unzip selects a directory's contents with a pattern; tar takes the directory's name.
  patterns=("${members[@]}")
  if [ "$kind" = tree ] && [[ "$archive" = *.zip ]]; then patterns=("${members[0]}/*"); fi
  mkdir "$tmp/x"
  case "$archive" in
    *.zip) unzip -q "$archive" "${patterns[@]}" -d "$tmp/x" ;;
    *.tar.gz) tar -xzf "$archive" -C "$tmp/x" --no-same-owner "${patterns[@]}" ;;
    *.tar.xz) tar -xJf "$archive" -C "$tmp/x" --no-same-owner "${patterns[@]}" ;;
    *) echo "dev-setup: unknown archive type for $name" >&2; exit 1 ;;
  esac
  case "$kind" in
    bin)
      install -m 0755 "$tmp/x/${members[0]}" "$bin_dir/$name"
      reports_version "$version" "$bin_dir/$name" "${version_args[@]:1}" || { echo "dev-setup: $name does not report version $version" >&2; exit 1; }
      where="$bin_dir"
      ;;
    files | tree)
      rm -rf -- "$tmp/new"
      if [ "$kind" = tree ]; then
        mv "$tmp/x/${members[0]}" "$tmp/new"
      else
        mkdir "$tmp/new"
        for member in "${members[@]}"; do install -m 0755 "$tmp/x/$member" "$tmp/new/$(basename "$member")"; done
      fi
      reports_version "$version" "$tmp/new/${version_args[0]}" "${version_args[@]:1}" || { echo "dev-setup: $name does not report version $version" >&2; exit 1; }
      echo "$sha256" >"$tmp/new/.bayandocs-sha256"
      rm -rf -- "$target"
      mv "$tmp/new" "$target"
      where="$target"
      ;;
    *) echo "dev-setup: unknown install kind for $name" >&2; exit 1 ;;
  esac
  rm -rf -- "$tmp"
  trap - EXIT
  echo "dev-setup: installed $name $version into $where"
done

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *)
    if [ -n "${GITHUB_PATH:-}" ]; then
      echo "$bin_dir" >>"$GITHUB_PATH"
    else
      echo "dev-setup: add $bin_dir to your PATH"
    fi
    ;;
esac
