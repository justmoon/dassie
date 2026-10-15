# Fedora 42 with systemd as PID 1, for the install tests (install_test.ts).
# Here the installer verifies with minisign rather than OpenSSL.
FROM fedora:42@sha256:99e203b80b1c3d8f7e161ec10a68fd02b081ef83a3963553e513c82846b97814
RUN dnf -y install systemd openssl xz tar shadow-utils minisign \
  && dnf clean all
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
