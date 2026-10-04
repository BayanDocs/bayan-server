# Deployment

bayan-server ships as one container image that needs nothing else: by default it keeps its data in an embedded SQLite database on a single volume (ADR-0015). Settings are described in [configuration.md](configuration.md).

## Build the image

```sh
docker build --build-arg BAYAN_BUILD_COMMIT="$(git rev-parse HEAD)" -t bayan-server .
```

The image is about 8.9 MB unpacked (about 4 MB compressed), 6.5 MB of which is the server itself. It contains one statically linked binary on the distroless `static` base image (no shell, no package manager, no C library), runs as the unprivileged user `nonroot` (uid and gid 65532), and has a health check that runs `bayan-server healthcheck`. Both base images are pinned by digest. Behind a TLS-inspecting proxy, pass the proxy's certificate bundle with `--secret id=extra-ca-certificates,src=<bundle.pem>`.

The image sets `BAYAN_LISTEN=0.0.0.0:8080`, `BAYAN_DATA_DIR=/data` and `BAYAN_LOG_FORMAT=json`.

## Run it

The recommended hardened form, with SQLite on a named volume:

```sh
docker run --detach --name bayan-server \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --volume bayan-data:/data \
  --publish 127.0.0.1:8080:8080 \
  bayan-server
```

- `--read-only` makes the container's root filesystem read-only; the server writes only to `/data`.
- `--cap-drop ALL` and `--security-opt no-new-privileges` remove Linux privileges the server never needs.
- A named volume takes the ownership of the image's `/data` (uid 65532) automatically. If you bind-mount a host directory instead, make it writable by uid 65532 first (`sudo chown 65532:65532 /srv/bayan-data`).
- Publishing on `127.0.0.1` keeps the port private to the host. Put a reverse proxy that terminates HTTPS (Caddy, nginx, Traefik) in front of it; the server itself speaks plain HTTP for now. The server closes connections that do not send a complete request within `BAYAN_HEADER_READ_TIMEOUT_SECS` (10 seconds), including idle keep-alive connections; if the proxy keeps idle connections to the server longer, raise that setting above the proxy's idle timeout.

With PostgreSQL, give the URL as a file so the password never appears in the environment:

```sh
printf '%s' 'postgres://bayan:<password>@db.internal:5432/bayan' > database-url
chmod 600 database-url
docker run --detach --name bayan-server \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --volume bayan-data:/data \
  --volume "$PWD/database-url:/run/secrets/database-url:ro" \
  --env BAYAN_DATABASE_URL_FILE=/run/secrets/database-url \
  --publish 127.0.0.1:8080:8080 \
  bayan-server
```

The connection to PostgreSQL is not encrypted yet, so the database must be reachable only over a trusted network (see [configuration.md](configuration.md#database)).

To serve the web app, mount its build read-only and point `BAYAN_WEB_DIR` at it: `--volume /srv/bayan-web:/web:ro --env BAYAN_WEB_DIR=/web`.

## Check it

```sh
curl http://127.0.0.1:8080/healthz      # ok
curl http://127.0.0.1:8080/readyz       # ready
curl http://127.0.0.1:8080/version
docker inspect --format '{{.State.Health.Status}}' bayan-server   # healthy
```

`scripts/container-smoke-test.sh <image>` runs all of this automatically, including checks that the root filesystem is read-only, every process runs as uid 65532 and the server stops cleanly; CI runs it on every pull request.

## Stop, upgrade, back up

- `docker stop bayan-server` sends `SIGTERM`; the server stops accepting connections, closes idle ones, lets requests in progress finish (up to `BAYAN_SHUTDOWN_GRACE_SECS`, 5 seconds by default), closes the database (at most 2 more seconds) and exits with status 0. That fits within Docker's default stop timeout of 10 seconds. If you raise the grace period, raise Docker's timeout to match (`docker stop --time`, `docker run --stop-timeout`, or `stop_grace_period` in Compose), or Docker kills the server before it has stopped cleanly.
- To upgrade, start the new image with the same volume and settings. Database migrations run automatically at startup.
- With SQLite, back up the `/data` volume while the server is stopped (online backup tooling comes with a later work package).

## Local development

`compose.yaml` starts the same hardened container for development: `docker compose up --build` (SQLite, port 8080) or `docker compose --profile postgres up --build` (adds PostgreSQL and a second server using it, port 8081). It is not a production recipe.
