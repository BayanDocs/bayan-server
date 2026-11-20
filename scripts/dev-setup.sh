#!/usr/bin/env bash
# Installs the tools bayan-server's checks need that are not part of the Rust toolchain, at exact versions verified against SHA-256 hashes (ADR-0017). Idempotent: a tool already installed at the pinned version is left alone. CI runs this script too, so local runs and CI use the same versions.
#
# Usage: scripts/dev-setup.sh [tool ...]   installs the named tools (default: all)
#
# cargo-deny and grype go into $BAYAN_TOOLS_BIN (default ~/.local/bin). The tools that run the MLS spike's tests in WebAssembly (spikes/mls) go into their own folders under $BAYAN_TOOLS_DIR (default ~/.local/share/bayandocs/server-tools), not on PATH, so they never replace a Node.js, browser or wasm-bindgen you use elsewhere; `cargo xtask mls-spike-wasm` finds them there.
#
# Pins change only in the monthly dependency session; keep cargo-deny, grype and Node.js equal to docs/scripts/cloud-environment-setup.sh, Node.js and the Chromium headless shell equal to bayan-web's scripts/dev-setup.sh, and wasm-bindgen equal to the wasm-bindgen crate in spikes/mls/Cargo.lock (the test runner refuses other versions). Every version was at least 24 hours old when pinned.
#   cargo-deny 0.21.0               released 2026-11-16  dependency policy checks in `cargo xtask verify`
#   grype 0.122.0                   released 2026-11-16  container image vulnerability scanner (CI)
#   wasm-bindgen 0.2.130            released 2026-10-30  wasm-bindgen and wasm-bindgen-test-runner, for the MLS spike's WebAssembly tests and size measurement
#   node 24.22.3                    released 2026-11-10  Node.js (only its `node` program), runs the WebAssembly tests in Node.js; hash from nodejs.org's signed SHASUMS256.txt, as in bayan-web
#   chromedriver 159.0.8154.6       released 2026-11-11  drives the browser for the WebAssembly tests in Chromium
#   chrome-headless-shell 159.0.8154.6  released 2026-11-11  Chromium's headless shell (Chrome for Testing), the same build bayan-web tests with
# The two Chrome for Testing archives were hashed from Google's storage.googleapis.com and confirmed identical on npmmirror (cdn.npmmirror.com/binaries/chrome-for-testing/…), a mirror run by a different organization, on 2026-11-20.
# Also needed, from the system: curl, sha256sum, tar with xz support, unzip; python3 and psql for `cargo xtask sqlx-prepare`; Docker for the container build and smoke test; and for chromedriver and the headless shell the libraries that Chromium uses (present on GitHub's Ubuntu runners; on Ubuntu 24.04, browser_packages below lists the packages that contain them, and when either program cannot start, this script says why and prints the command that installs them).
set -euo pipefail

bin_dir="${BAYAN_TOOLS_BIN:-$HOME/.local/bin}"
tools_dir="${BAYAN_TOOLS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/bayandocs/server-tools}"

# name | version | URL | SHA-256 of the archive | what to install | command (relative to the install location) that prints the version
# What to install: "bin:<path>" copies that file from the archive into $bin_dir; "files:<path>,<path>,..." copies those files into $tools_dir/<name>/; "tree:<directory>" installs that directory of the archive as $tools_dir/<name>/.
tools=(
  "cargo-deny|0.21.0|https://github.com/EmbarkStudios/cargo-deny/releases/download/0.21.0/cargo-deny-0.21.0-x86_64-unknown-linux-musl.tar.gz|a2170b854ff697e79e39c4c538981c9c1bdd803addbae6719a4cb66b4585a903|bin:cargo-deny-0.21.0-x86_64-unknown-linux-musl/cargo-deny|cargo-deny --version"
  "grype|0.122.0|https://github.com/anchore/grype/releases/download/v0.122.0/grype_0.122.0_linux_amd64.tar.gz|e697e6f7224637df87e267e4318cc027feae99a8e1b80280241d32b9ee5d4920|bin:grype|grype --version"
  "wasm-bindgen|0.2.130|https://github.com/wasm-bindgen/wasm-bindgen/releases/download/0.2.130/wasm-bindgen-0.2.130-x86_64-unknown-linux-musl.tar.gz|9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f|files:wasm-bindgen-0.2.130-x86_64-unknown-linux-musl/wasm-bindgen,wasm-bindgen-0.2.130-x86_64-unknown-linux-musl/wasm-bindgen-test-runner|wasm-bindgen --version"
  "node|24.22.3|https://nodejs.org/dist/v24.22.3/node-v24.22.3-linux-x64.tar.xz|4f2b175a6d53a5c617a80dba2676238a0688cef28c6da4eba35c7d844e66fcfb|files:node-v24.22.3-linux-x64/bin/node|node --version"
  "chromedriver|159.0.8154.6|https://storage.googleapis.com/chrome-for-testing-public/159.0.8154.6/linux64/chromedriver-linux64.zip|7c6e8e13d6ec61c2774db650e4f15dfed0c31ed98fa56bd89af3ad0b0820bc44|files:chromedriver-linux64/chromedriver|chromedriver --version"
  "chrome-headless-shell|159.0.8154.6|https://storage.googleapis.com/chrome-for-testing-public/159.0.8154.6/linux64/chrome-headless-shell-linux64.zip|77a6442fd3c8acd3bef9e1067fc26f5ef464277d22f0e8cb58dc3e4077a3e458|tree:chrome-headless-shell-linux64|chrome-headless-shell --version"
)

# The Ubuntu 24.04 packages that contain the shared libraries chromedriver and the headless shell link and a bare Ubuntu 24.04 installation lacks; apt adds what those libraries need in turn. Without them neither program starts, so this script cannot check their versions: it then says why and prints the command that installs these packages, but never runs apt or sudo itself. Checked on 2026-11-20 in a bare Ubuntu 24.04 container, where with only these packages added both programs started and the MLS spike's tests passed in Chromium; check the list again with ldd when the two programs move to a new version.
browser_packages=(
  libasound2t64 libatk-bridge2.0-0t64 libatk1.0-0t64 libatspi2.0-0t64 libdbus-1-3 libexpat1 libgbm1 libglib2.0-0t64 libnspr4
  libnss3 libx11-6 libxcb1 libxcomposite1 libxdamage1 libxext6 libxfixes3 libxkbcommon0 libxrandr2
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

# Says why chromedriver or the headless shell (`name`; `command` is the program and its arguments) did not report its version: what the program printed, the shared libraries it needs that ldd cannot find, and the command that installs the Ubuntu packages in browser_packages. ldd may run the program to list its libraries (see ldd(1)), which adds no risk: the program, from an archive whose SHA-256 was checked, has just been run anyway.
explain_browser_failure() {
  local name="$1"
  local -a printed missing
  shift
  mapfile -t printed < <("$@" 2>&1 | head -n 20)
  if [ "${#printed[@]}" -gt 0 ]; then
    echo "dev-setup: $name printed:" >&2
    printf '  %s\n' "${printed[@]}" >&2
  fi
  if command -v ldd >/dev/null; then
    mapfile -t missing < <(ldd "$1" 2>/dev/null | awk '/=> not found$/ { print $1 }')
    if [ "${#missing[@]}" -gt 0 ]; then
      echo "dev-setup: $name needs these shared libraries, which ldd cannot find: ${missing[*]}" >&2
    fi
  fi
  echo "dev-setup: chromedriver and the headless shell need the shared libraries that Chromium uses. On Ubuntu 24.04, browser_packages in scripts/dev-setup.sh lists the packages that contain them; install them with" >&2
  echo "  sudo apt install ${browser_packages[*]}" >&2
  echo "dev-setup: then run this script again. On another system, install the packages that contain the missing libraries." >&2
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
      reports_version "$version" "$tmp/new/${version_args[0]}" "${version_args[@]:1}" || {
        echo "dev-setup: $name does not report version $version" >&2
        case "$name" in chromedriver | chrome-headless-shell) explain_browser_failure "$name" "$tmp/new/${version_args[0]}" "${version_args[@]:1}" ;; esac
        exit 1
      }
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
