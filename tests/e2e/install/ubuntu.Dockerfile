# Ubuntu 24.04 with systemd as PID 1, for the install tests (install_test.ts).
# Here the installer fetches with wget rather than curl.
FROM ubuntu:24.04@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55
RUN apt-get update \
  && apt-get install -y --no-install-recommends systemd systemd-sysv ca-certificates wget openssl \
  && apt-get clean && rm -rf /var/lib/apt/lists/*
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
