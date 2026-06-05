# Systemd-enabled "host" for the remediation demo: runs ravnd + the privileged
# ravn-actuator as systemd units (so the failed-unit tap and `systemctl`-based
# healing work for real), plus a `flaky` unit you can kill to trigger a fix.
# Build context is the repo root.
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p ravn-agent -p ravn-actuator

FROM debian:bookworm
ENV container=docker
RUN apt-get update && apt-get install -y --no-install-recommends \
      systemd systemd-sysv dbus ca-certificates curl procps \
    && rm -rf /var/lib/apt/lists/*
# Drop container-pointless units so boot is quick and clean.
RUN find /etc/systemd/system /lib/systemd/system -path '*.wants/*' \
      \( -name '*getty*' -o -name '*systemd-update-utmp*' \
         -o -name '*systemd-tmpfiles*' -o -name '*systemd-modules-load*' \
         -o -name '*e2scrub*' \) -delete 2>/dev/null || true

COPY --from=builder /build/target/release/ravnd /usr/local/bin/ravnd
COPY --from=builder /build/target/release/ravn-actuator /usr/local/bin/ravn-actuator
COPY demo/host-agent/units/*.service /etc/systemd/system/
COPY demo/command_pubkey.b64 /etc/ravn/command_pubkey.b64
RUN printf 'RAVN_COMMAND_PUBKEY=%s\n' "$(cat /etc/ravn/command_pubkey.b64)" > /etc/ravn/actuator.env \
    && systemctl enable ravn-actuator.service ravnd.service flaky.service

STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
