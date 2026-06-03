# Builds both Ravn binaries (ravn-server, ravnd) into one slim runtime image.
# Used by docker-compose for a one-command demo on non-Nix hosts.
# (Nix users: prefer the reproducible images — `nix build .#ravn-server-image`.)
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p ravn-server -p ravn-agent

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/ravn-server /usr/local/bin/ravn-server
COPY --from=builder /build/target/release/ravnd /usr/local/bin/ravnd
# Default to the control plane; the agent service overrides the command.
CMD ["ravn-server"]
