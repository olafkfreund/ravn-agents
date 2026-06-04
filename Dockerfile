# Builds the Ravn binaries (ravn-server, ravnd, and the K8s controller +
# node-agent) into one slim runtime image. Used by docker-compose for a
# one-command demo and by the kind/k3d e2e (#60) on non-Nix hosts.
# (Nix users: prefer the reproducible images — `nix build .#ravn-server-image`.)
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p ravn-server -p ravn-agent -p ravn-k8s

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/ravn-server /usr/local/bin/ravn-server
COPY --from=builder /build/target/release/ravnd /usr/local/bin/ravnd
COPY --from=builder /build/target/release/ravn-controller /usr/local/bin/ravn-controller
COPY --from=builder /build/target/release/ravn-node-agent /usr/local/bin/ravn-node-agent
# Default to the control plane; other services override the command.
CMD ["ravn-server"]
