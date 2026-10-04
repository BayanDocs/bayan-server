# Container image of the BayanDocs server (ADR-0015): one static binary on a distroless base, running as a non-root user with a read-only root filesystem and a writable /data volume.
#
# Build:  docker build --build-arg BAYAN_BUILD_COMMIT="$(git rev-parse HEAD)" -t bayan-server .
# Run:    docker run --read-only -v bayan-data:/data -p 8080:8080 bayan-server
#
# Base images are pinned by digest and were at least 24 hours old when pinned (ADR-0017); they change only in the monthly dependency session.
# The builder's Rust version must equal rust-toolchain.toml.

# rust:1.99.0-alpine3.23, created 2026-10-01T23:55Z.
FROM docker.io/library/rust:1.99.0-alpine3.23@sha256:e8dde6f7bd5650824aeede43b4652cdaf07ee5277488f300bf5b22e867d634b8 AS builder
WORKDIR /src
# Alpine's Rust targets musl and links statically, so the binary needs no C library at run time. gcc and musl-dev in the image compile the bundled SQLite.
# rust-toolchain.toml is kept outside /src so rustup uses the image's toolchain instead of downloading components; the check below makes sure the versions agree.
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN test "$(rustc --version | cut -d ' ' -f 2)" = "$(sed -n 's/^channel = "\(.*\)"/\1/p' /tmp/rust-toolchain.toml)" \
 || { echo "the builder image's Rust does not match rust-toolchain.toml" >&2; exit 1; }
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY crates crates
COPY xtask xtask
ARG BAYAN_BUILD_COMMIT=unknown
# Builds behind a TLS-inspecting proxy can pass its certificate bundle with `--secret id=extra-ca-certificates,src=<bundle.pem>`; Cargo then trusts that bundle instead of the default one. Without the secret nothing changes.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    --mount=type=secret,id=extra-ca-certificates,required=false \
    if [ -s /run/secrets/extra-ca-certificates ]; then export CARGO_HTTP_CAINFO=/run/secrets/extra-ca-certificates; fi \
 && BAYAN_BUILD_COMMIT="${BAYAN_BUILD_COMMIT}" cargo build --locked --release --package bayan-server \
 && cp target/release/bayan-server /usr/local/bin/bayan-server
# The data directory, owned by the distroless "nonroot" user (65532), becomes the volume's initial content.
RUN mkdir -m 0700 /data && chown 65532:65532 /data

# gcr.io/distroless/static-debian13:nonroot, uploaded 2026-09-13. No shell, no package manager, no C library.
FROM gcr.io/distroless/static-debian13:nonroot@sha256:e2e927ec666bae08560abb3c55d0659eceabb657f56b6782ab500a9fc7f555e3
ARG BAYAN_BUILD_COMMIT=unknown
LABEL org.opencontainers.image.title="bayan-server" \
      org.opencontainers.image.description="BayanDocs collaboration server: a zero-knowledge relay for end-to-end-encrypted documents" \
      org.opencontainers.image.source="https://github.com/BayanDocs/bayan-server" \
      org.opencontainers.image.url="https://github.com/BayanDocs/bayan-server" \
      org.opencontainers.image.documentation="https://github.com/BayanDocs/bayan-server/blob/main/docs/configuration.md" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later" \
      org.opencontainers.image.revision="${BAYAN_BUILD_COMMIT}" \
      org.opencontainers.image.base.name="gcr.io/distroless/static-debian13:nonroot"
COPY --from=builder /usr/local/bin/bayan-server /usr/local/bin/bayan-server
COPY --from=builder --chown=65532:65532 /data /data
# Self-test: the binary runs in the final image (it is static, and the image has no C library).
RUN ["/usr/local/bin/bayan-server", "version"]
ENV BAYAN_LISTEN=0.0.0.0:8080 \
    BAYAN_DATA_DIR=/data \
    BAYAN_LOG_FORMAT=json
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --start-interval=1s --retries=3 CMD ["/usr/local/bin/bayan-server", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/bayan-server"]
CMD ["serve"]
