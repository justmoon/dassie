# Debian 12 with systemd as PID 1, for the install tests (install_test.ts):
# what a small server has, and the tools the installer asks for.
FROM debian:12-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587
RUN apt-get update \
  && apt-get install -y --no-install-recommends systemd systemd-sysv ca-certificates curl openssl xz-utils \
  && apt-get clean && rm -rf /var/lib/apt/lists/*
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
