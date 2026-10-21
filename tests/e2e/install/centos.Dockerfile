# CentOS Stream 9 with systemd as PID 1, for the install tests
# (install_test.ts). It has no xz, so the installer takes the gzip bundle.
FROM quay.io/centos/centos:stream9@sha256:086f25bff2b5e44eaa6953c8165cf6c4dcb3af8018b37053b510c7d5d796613c
RUN dnf -y install systemd openssl tar shadow-utils \
  && dnf clean all \
  && ! command -v xz
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
