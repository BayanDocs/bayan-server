#!/usr/bin/env bash
# Installs the tools bayan-server's checks need that are not part of the Rust toolchain, at exact versions verified against SHA-256 hashes (ADR-0017). Idempotent: a tool already installed at the pinned version is left alone. CI runs this script too, so local runs and CI use the same versions.
#
# Usage: scripts/dev-setup.sh [tool ...]   installs the named tools (default: all) into $BAYAN_TOOLS_BIN (default ~/.local/bin)
#
# Pins change only in the monthly dependency session; keep them equal to docs/scripts/cloud-environment-setup.sh. Every version was at least 24 hours old when pinned.
#   cargo-deny 0.20.2  released 2026-07-09  dependency policy checks in `cargo xtask verify`
#   grype 0.120.0      released 2026-10-02  container image vulnerability scanner (CI)
# Also needed, from the system: curl, sha256sum, tar; python3 and psql for `cargo xtask sqlx-prepare`; Docker for the container build and smoke test.
set -euo pipefail

bin_dir="${BAYAN_TOOLS_BIN:-$HOME/.local/bin}"

# name, version, URL, SHA-256 of the archive, path of the binary inside the archive, command that prints the version
tools=(
  "cargo-deny|0.20.2|https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz|9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f|cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny|--version"
  "grype|0.120.0|https://github.com/anchore/grype/releases/download/v0.120.0/grype_0.120.0_linux_amd64.tar.gz|a5a1218dce63acdac152a6b3b5bb366e7267e36f4069848cf455543b3fa5700e|grype|--version"
)

if [ "$(uname -s)-$(uname -m)" != "Linux-x86_64" ]; then
  echo "dev-setup: only Linux x86-64 is supported; install the versions listed in this script by other means" >&2
  exit 1
fi

wanted=("$@")
for name in "${wanted[@]}"; do
  printf '%s\n' "${tools[@]}" | grep -q "^$name|" || { echo "dev-setup: unknown tool $name" >&2; exit 1; }
done

mkdir -p "$bin_dir"
for entry in "${tools[@]}"; do
  IFS='|' read -r name version url sha256 member version_flag <<<"$entry"
  if [ "${#wanted[@]}" -gt 0 ] && ! printf '%s\n' "${wanted[@]}" | grep -qx "$name"; then
    continue
  fi
  # Already installed at the pinned version (here or elsewhere on PATH)?
  for candidate in "$bin_dir/$name" "$(command -v "$name" || true)"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ] && "$candidate" "$version_flag" 2>/dev/null | grep -qE "(^| |v)$version( |$)"; then
      echo "dev-setup: $name $version already installed ($candidate)"
      continue 2
    fi
  done
  tmp="$(mktemp -d)"
  trap 'rm -rf -- "$tmp"' EXIT
  echo "dev-setup: installing $name $version"
  curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 --output "$tmp/archive.tar.gz" "$url"
  echo "$sha256  $tmp/archive.tar.gz" | sha256sum --check --quiet - || { echo "dev-setup: checksum mismatch for $name" >&2; exit 1; }
  tar -xzf "$tmp/archive.tar.gz" -C "$tmp" --no-same-owner "$member"
  install -m 0755 "$tmp/$member" "$bin_dir/$name"
  rm -rf -- "$tmp"
  trap - EXIT
  "$bin_dir/$name" "$version_flag" >/dev/null
  echo "dev-setup: installed $name $version into $bin_dir"
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
