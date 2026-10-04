# Configuration

The server is configured with environment variables, optionally combined with a TOML file. Every setting has a safe default, so `bayan-server` with no configuration at all starts a server on `127.0.0.1:8080` with an SQLite database in `./data`. The container image changes three defaults (see [deployment.md](deployment.md)).

## Where settings come from

From lowest to highest precedence:

1. Built-in defaults (the table below).
2. A TOML file, if `BAYAN_CONFIG_FILE` names one. Keys are the lower-case names in the table.
3. `BAYAN_*` environment variables.

So an environment variable always wins over the file, and the file wins over the default.

**Secrets from files.** A setting that may contain a secret also has a `_FILE` form whose value is the path of a file holding the secret (`BAYAN_DATABASE_URL_FILE`, or `database_url_file` in the TOML file). This keeps passwords out of the process environment, where other tools may print them, and works with Docker and Kubernetes secrets. One trailing line ending in the file is ignored. Setting both forms of the same setting is an error.

**Strict validation.** The server refuses to start, with a message naming the problem, when a value is invalid or out of range, when an environment variable starting with `BAYAN_` is not one of the settings below, or when the TOML file contains an unknown key. This catches typos that would otherwise silently fall back to a default. Error messages name the setting, and for the TOML file the line number and the key, but never repeat a value, a file path or a file's contents, because a secret may have been put in the wrong place by mistake. For example, a URL accidentally put under `database_max_connections` is reported as `invalid configuration file (BAYAN_CONFIG_FILE) at line 1, key `database_max_connections`: wrong kind of value; expected a whole number`. The exit code for configuration errors is 78.

## Settings

| Environment variable | TOML key | Default | Meaning |
|---|---|---|---|
| `BAYAN_CONFIG_FILE` | — | none | Path of the optional TOML configuration file. At most 64 KiB. |
| `BAYAN_LISTEN` | `listen` | `127.0.0.1:8080` | IP address and port to listen on, such as `0.0.0.0:8080` or `[::]:8080`. Host names are not accepted. |
| `BAYAN_DATA_DIR` | `data_dir` | `data` (relative to the working directory) | Directory for persistent data. The SQLite database is `bayan.sqlite3` inside it. Created with owner-only permissions if missing. |
| `BAYAN_DATABASE_URL` | `database_url` | none: use SQLite | URL of a PostgreSQL database, `postgres://user:password@host:port/database`. Leave unset to use the built-in SQLite database. Only `postgres://` and `postgresql://` URLs are accepted. |
| `BAYAN_DATABASE_URL_FILE` | `database_url_file` | none | File containing the PostgreSQL URL (preferred over `BAYAN_DATABASE_URL`, because the URL contains a password). At most 64 KiB. |
| `BAYAN_DATABASE_MAX_CONNECTIONS` | `database_max_connections` | `10` | Maximum open database connections, 1 to 1000. |
| `BAYAN_WEB_DIR` | `web_dir` | none | Directory of static files to serve, normally the built web app. Requests that match no server route are answered from this directory (`index.html` for directories). Unset: no static files. |
| `BAYAN_LOG_FORMAT` | `log_format` | `text` | `text` for human-readable lines, or `json` for one JSON object per line. |
| `BAYAN_LOG_LEVEL` | `log_level` | `info` | `error`, `warn`, `info`, `debug` or `trace`. |
| `BAYAN_MAX_REQUEST_BODY_BYTES` | `max_request_body_bytes` | `1048576` (1 MiB) | Largest accepted request body; larger requests get `413 Payload Too Large`. 1 byte to 1 GiB. |
| `BAYAN_REQUEST_TIMEOUT_SECS` | `request_timeout_secs` | `30` | Once a request's headers have arrived, the time it may take (including reading its body) before it is answered with `408 Request Timeout`. 1 to 3600. |
| `BAYAN_HEADER_READ_TIMEOUT_SECS` | `header_read_timeout_secs` | `10` | Time a connection gets to send a complete request line and headers; connections that do not are closed without an answer. The same limit closes connections that stay idle, before their first request or between keep-alive requests. Behind a reverse proxy that keeps idle connections to the server open longer than this, raise it above the proxy's idle timeout (or lower the proxy's) to avoid the proxy reusing a connection the server is just closing. 1 to 300. |
| `BAYAN_SHUTDOWN_GRACE_SECS` | `shutdown_grace_secs` | `5` | After `SIGTERM` or `SIGINT`, how long requests in progress may take to finish before the server stops anyway; closing the database afterwards takes at most 2 more seconds. Keep it below your orchestrator's stop timeout (Docker: 10 seconds, Kubernetes: 30 seconds), or the server is killed before it can stop cleanly. 0 to 3600. |

Example TOML file:

```toml
listen = "0.0.0.0:8080"
data_dir = "/data"
database_url_file = "/run/secrets/database-url"
log_format = "json"
web_dir = "/srv/bayan-web"
```

## Database

- **SQLite (default):** zero setup. The database file lives in `BAYAN_DATA_DIR`; back up that directory. SQLite runs in write-ahead-logging mode with full synchronization, and keeps temporary tables in memory so it works with a read-only root filesystem.
- **PostgreSQL (optional, for larger deployments):** set `BAYAN_DATABASE_URL_FILE` (or `BAYAN_DATABASE_URL`). The database must exist; the server creates its tables.
- **Migrations** are applied automatically at startup, in both cases. A database that has migrations this server version does not know (for example after a downgrade) is refused.
- **No TLS to PostgreSQL yet.** The decision is made ([ADR-0028](https://github.com/BayanDocs/docs/blob/main/adr/0028-server-tls.md)): the server will use rustls with the aws-lc-rs provider, with certificate verification on by default and post-quantum key exchange, and a later work package (SRV-102) adds it. Until then, run PostgreSQL on the same host or a private network, and do not use `sslmode=require` (it fails to connect).
- **`PG*` variables and password files are read by the driver, outside the `BAYAN_*` checks.** Like PostgreSQL's own tools, the database driver fills in anything the URL leaves out from the standard environment variables `PGHOST`, `PGHOSTADDR`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`, `PGSSLMODE`, `PGSSLROOTCERT`, `PGSSLCERT`, `PGSSLKEY`, `PGAPPNAME` and `PGOPTIONS`, and, when the URL has no password, from a password file (`PGPASSFILE`, or `.pgpass` in the home directory). The server's strict validation does not see these: a misspelled `PG*` variable is silently ignored, and `PGPASSWORD` puts the password into the environment, which `BAYAN_DATABASE_URL_FILE` exists to avoid. Put the whole connection, password included, in the URL file and leave the `PG*` variables unset. (The application name is always `bayan-server`, whatever `PGAPPNAME` says.) In the container the root filesystem is read-only, so no password file exists unless you mount one.

## Logs

Logs go to standard error. They contain operational facts only: for each request its ID, method, matched route template (for example `/readyz`; anything else is logged as `other` or `-`), status and duration. They never contain request or response bodies, header values (including `Authorization` and `Cookie`), query strings, the paths clients requested, document content, titles, file names or user identifiers. Log events from dependencies that can contain secrets are switched off at every level (today one: the database driver's report of a malformed password-file line, which would quote the line). The tests `tests/logging_text.rs` and `tests/logging_json.rs` in `crates/bayan-server` enforce all of this at the `trace` level.

Every response carries an `x-request-id` header with the ID from the log, so a user's report can be matched to the log line. IDs are 32 hexadecimal digits that look random: unlike a counter, they reveal nothing about how many requests the server has handled. IDs sent by clients are replaced.

## HTTP endpoints

| Path | Purpose |
|---|---|
| `GET /healthz` | Liveness: `200 ok` while the process serves HTTP. Does not touch the database. |
| `GET /readyz` | Readiness: `200 ready` when the database is reachable and migrated, otherwise `503 not ready`. |
| `GET /version` | `{"name":"bayan-server","version":"…","commit":"…"}`. |

Every response carries the security headers of ADR-0014: a strict Content Security Policy (same-origin scripts plus WebAssembly only, Trusted Types required), `Integrity-Policy` (scripts need Subresource Integrity), cross-origin isolation, `nosniff`, `no-referrer`, frame denial and a restrictive `Permissions-Policy`. The exact values are in `crates/bayan-server/src/http/headers.rs`.

## Command line

`bayan-server` (or `bayan-server serve`) runs the server. `bayan-server healthcheck` exits with status 0 when the server configured by the same settings answers `GET /healthz` with `200 OK` on the loopback interface; the container image uses it as its health check because the image has no shell or `curl`. `bayan-server version` prints the version.
