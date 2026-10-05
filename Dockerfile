# Container image of the BayanDocs server (ADR-0015): one static binary in an otherwise empty image, running as a non-root user with a read-only root filesystem and a writable /data volume.
#
# Build:  docker build --build-arg BAYAN_BUILD_COMMIT="$(git rev-parse HEAD)" -t bayan-server .
# Run:    docker run --read-only -v bayan-data:/data -p 8080:8080 bayan-server
#
# The builder image is pinned by digest and was at least 24 hours old when pinned (ADR-0017); it changes only in the monthly dependency session. The final image has no base image.
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
# Everything else the final image contains: an empty data directory and user and group files that name uid and gid 65532 "nonroot", as tools that show user names expect.
RUN mkdir -p /rootfs/etc /rootfs/data \
 && printf 'root:x:0:0:root:/root:/sbin/nologin\nnonroot:x:65532:65532:nonroot:/nonexistent:/sbin/nologin\n' >/rootfs/etc/passwd \
 && printf 'root:x:0:\nnonroot:x:65532:\n' >/rootfs/etc/group \
 && chmod 0644 /rootfs/etc/passwd /rootfs/etc/group

# An empty base: the image holds only the files copied below. It has no shell, package manager, C library or operating-system packages, so there is nothing to patch besides the server. The ADR-0017 license allowlist covers every file in the containers we distribute: the binary contains only our code, our crates' dependencies, Rust's standard library, musl and LLVM's libunwind, all under allowed licenses. Anything added here must be allowed too; scripts/container-smoke-test.sh lists the files the image may contain.
FROM scratch
ARG BAYAN_BUILD_COMMIT=unknown
LABEL org.opencontainers.image.title="bayan-server" \
      org.opencontainers.image.description="BayanDocs collaboration server: a zero-knowledge relay for end-to-end-encrypted documents" \
      org.opencontainers.image.source="https://github.com/BayanDocs/bayan-server" \
      org.opencontainers.image.url="https://github.com/BayanDocs/bayan-server" \
      org.opencontainers.image.documentation="https://github.com/BayanDocs/bayan-server/blob/main/docs/configuration.md" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later" \
      org.opencontainers.image.revision="${BAYAN_BUILD_COMMIT}"
COPY --from=builder /rootfs/etc/passwd /rootfs/etc/group /etc/
# The data directory belongs to the unprivileged user 65532 and only it may enter; a new named volume takes this owner and mode.
COPY --from=builder --chown=65532:65532 --chmod=0700 /rootfs/data /data
COPY --from=builder /usr/local/bin/bayan-server /usr/local/bin/bayan-server
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
