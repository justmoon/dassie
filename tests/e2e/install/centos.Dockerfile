# CentOS Stream 9 with systemd as PID 1, for the install tests
# (install_test.ts): the smallest of the four, with tar and OpenSSL alone.
FROM quay.io/centos/centos:stream9@sha256:6f8045fc4f552544d8a401d61848bafc8d06ca7c11897c5b54a2c37fc419a20a
RUN dnf -y install systemd openssl tar shadow-utils \
  && dnf clean all
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
