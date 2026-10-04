#!/usr/bin/env bash
# Smoke test for the bayan-server container image (SRV-001). Usage: scripts/container-smoke-test.sh <image>
#
# Runs the image the way operators should: read-only root filesystem, all capabilities dropped, no privilege escalation, a named volume at /data. Then it checks that the container becomes healthy through its own HEALTHCHECK, answers /healthz, /readyz and /version, runs as the non-root user, stored its SQLite database on the volume, cannot write to its root filesystem, writes JSON logs, and stops cleanly on SIGTERM within Docker's default stop timeout even while a client holds a half-sent request open. It prints the image size at the end.
set -euo pipefail

image="${1:?usage: $0 <image>}"
name="bayan-smoke-$$"
volume="bayan-smoke-data-$$"
holder=""

cleanup() {
  if [ -n "$holder" ]; then kill "$holder" 2>/dev/null || true; fi
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker volume rm "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  docker logs "$name" >&2 2>&1 || true
  exit 1
}
pass() { echo "ok: $*"; }

docker volume create "$volume" >/dev/null
docker run --detach --name "$name" \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --volume "$volume:/data" --publish 127.0.0.1::8080 \
  "$image" >/dev/null

# Wait up to 60 seconds for the image's HEALTHCHECK (`bayan-server healthcheck`) to report healthy.
for _ in $(seq 1 60); do
  [ "$(docker inspect --format '{{.State.Running}}' "$name")" = true ] || fail "the container exited"
  status="$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{end}}' "$name")"
  [ "$status" = healthy ] && break
  sleep 1
done
[ "$status" = healthy ] || fail "the container did not become healthy (status: ${status:-none})"
pass "container health check reports healthy"

port="$(docker port "$name" 8080/tcp | head -n 1 | sed 's/.*://')"
base="http://127.0.0.1:${port}"
[ "$(curl --fail --silent --show-error "$base/healthz")" = ok ] || fail "/healthz"
pass "/healthz answers ok"
[ "$(curl --fail --silent --show-error "$base/readyz")" = ready ] || fail "/readyz"
pass "/readyz answers ready (SQLite database migrated)"
version="$(curl --fail --silent --show-error "$base/version")"
echo "$version" | grep -q '"name":"bayan-server"' || fail "/version returned: $version"
pass "/version answers $version"
headers="$(curl --fail --silent --show-error --head "$base/healthz")"
echo "$headers" | grep -qi '^content-security-policy: ' || fail "missing Content-Security-Policy header"
pass "security headers present"

# AC-2: non-root user and read-only root filesystem.
[ "$(docker inspect --format '{{.HostConfig.ReadonlyRootfs}}' "$name")" = true ] || fail "root filesystem is not read-only"
pass "root filesystem is mounted read-only"
[ "$(docker inspect --format '{{.Config.User}}' "$name")" = "65532:65532" ] || fail "image user is not 65532:65532"
uids="$(docker top "$name" -o pid,uid,gid,comm | tail -n +2)"
[ -n "$uids" ] || fail "no processes listed"
echo "$uids" | awk '{ if ($2 != 65532 || $3 != 65532) exit 1 }' || fail "a process runs as another user: $uids"
pass "every process runs as uid 65532, gid 65532 (nonroot): $(echo "$uids" | tr -s ' ' | tr '\n' ';')"
if docker cp "$0" "$name:/smoke-test-write" 2>/dev/null; then fail "wrote a file to the root filesystem"; fi
pass "writing to the root filesystem is refused"
docker cp "$name:/data/bayan.sqlite3" - >/dev/null || fail "no SQLite database on the /data volume"
pass "the SQLite database lives on the /data volume"

# Logs are JSON (the image sets BAYAN_LOG_FORMAT=json).
logs="$(docker logs "$name" 2>&1)"
printf '%s\n' "$logs" | python3 -c 'import json, sys; [json.loads(line) for line in sys.stdin if line.strip()]' \
  || fail "log lines are not JSON"
pass "logs are JSON lines"

# Graceful shutdown on SIGTERM, with Docker's default stop timeout (10 seconds), while a client holds half a request open: the server must still stop by itself instead of being killed (exit code 137).
python3 - "$port" <<'PY' &
import socket, sys, time
connection = socket.create_connection(("127.0.0.1", int(sys.argv[1])))
connection.sendall(b"GET /healthz HTTP/1.1\r\nHost: smoke-test\r\n")
time.sleep(60)
PY
holder=$!
sleep 1
started=$(date +%s)
docker stop "$name" >/dev/null
seconds=$(( $(date +%s) - started ))
kill "$holder" 2>/dev/null || true
holder=""
exit_code="$(docker inspect --format '{{.State.ExitCode}}' "$name")"
[ "$exit_code" = 0 ] || fail "exit code $exit_code after SIGTERM (137 means Docker had to kill it)"
logs="$(docker logs "$name" 2>&1)"
grep -q '"message":"stopped"' <<<"$logs" || fail "no clean shutdown in the logs"
pass "stops cleanly on SIGTERM within Docker's default timeout, despite a half-sent request (exit code 0 after ${seconds} s)"

# Image size, measured the same way with either Docker image store: the total size of the files in the image's filesystem. (`docker image inspect` reports the unpacked size with Docker's classic store but the compressed size with the containerd store.)
sizer="$(docker create "$image")"
size="$(docker export "$sizer" | tar -tvf - | awk '{ total += $3 } END { print total }')"
binary="$(docker export "$sizer" | tar -tvf - usr/local/bin/bayan-server | awk '{ print $3 }')"
docker rm "$sizer" >/dev/null
echo "image size: $size bytes unpacked ($(( size / 1024 / 1024 )) MiB), of which the server binary is $binary bytes"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  printf '### Container image\n\n| Image | Unpacked size | Server binary |\n|---|---|---|\n| %s | %s bytes | %s bytes |\n' "$image" "$size" "$binary" >>"$GITHUB_STEP_SUMMARY"
fi
