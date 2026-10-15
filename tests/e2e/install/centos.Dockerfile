# CentOS Stream 9 with systemd as PID 1, for the install tests
# (install_test.ts). It has no xz, so the installer takes the gzip bundle.
FROM quay.io/centos/centos:stream9@sha256:4fdfb367402535024cd7703a67e4549ca3b0a0ee05081e06d89e24dce3a3827a
RUN dnf -y install systemd openssl tar shadow-utils \
  && dnf clean all \
  && ! command -v xz
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
