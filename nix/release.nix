# Release builds (spec D11, D14): the wallet, the `dassie` binary for
# x64 and arm64 Linux, statically linked against musl, its bundles, and the
# mirror tree a release publishes.
#
# Everything is built on x86_64-linux: arm64 is cross-compiled, and the
# wallet is built with the npm packages for the build host's platform
# (nix/pnpm.nix).
{
  lib,
  pkgs,
  nixpkgs,
  rust-overlay,
  crane,
  self,
  src,
  identity-wasm,
}:
let
  # A release's version is dassie-cli's. A canary's names when the commit it
  # was built from was made, in UTC, and the commit:
  # `canary-2026-10-15-041530-15bd7aa`. A source without a commit gives
  # "unknown", which is no version: such a tree cannot be signed.
  releaseVersion = (lib.importTOML ../crates/dassie-cli/Cargo.toml).package.version;
  canaryVersion =
    let
      part = start: length: lib.substring start length self.lastModifiedDate;
      commit = self.shortRev or self.dirtyShortRev or "unknown";
    in
    "canary-${part 0 4}-${part 4 2}-${part 6 2}-${part 8 6}-${commit}";

  # The wallet, as `node --run build:wallet` builds it, offline: the npm
  # and JSR packages of the wallet and the members it uses, each fetched
  # as pnpm-lock.yaml pins it (nix/pnpm.nix). Vite reads the wallet's
  # tsconfig.json, which extends the base one.
  pnpm = import ./pnpm.nix { inherit lib pkgs; };
  wallet = pkgs.stdenvNoCC.mkDerivation {
    name = "dassie-wallet";
    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        pnpm.manifests
        ../tsconfig.base.json
        ../apps/wallet
        ../packages/identity-wasm
        ../packages/rpc
        ../packages/ui
      ];
    };
    nativeBuildInputs = [
      pkgs.nodejs_26
      pkgs.pnpm_11
    ];
    buildPhase = ''
      export HOME=$TMPDIR
      mkdir -p packages/identity-wasm/gen
      cp -r ${identity-wasm}/web packages/identity-wasm/gen/
    ''
    + pnpm.install "apps/wallet"
    + ''
      cd apps/wallet
      node --run build
    '';
    installPhase = "cp -r dist $out";
  };

  # The release key, once the owner has made it: the binaries trust it and
  # the installer prints it (D11). Until then they trust none, and the
  # release workflow signs nothing.
  releaseKey = if builtins.pathExists ../resources/minisign.pub then ../resources/minisign.pub else null;
  # The test key of testdata/release, for the install tests' releases.
  testKey = ../testdata/release/test.pub;

  # `dassie` for one platform, with the wallet built in, trusting `key`.
  # The install tests build theirs in a cheaper profile; the code is the
  # same.
  dassieFor =
    {
      crossSystem,
      target,
      version,
      key ? releaseKey,
      profile ? "release",
    }:
    let
      crossPkgs = import nixpkgs {
        localSystem = "x86_64-linux";
        inherit crossSystem;
        overlays = [ rust-overlay.overlays.default ];
      };
      craneLib = (crane.mkLib crossPkgs).overrideToolchain (
        p: (p.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml).override { targets = [ target ]; }
      );
      common = {
        inherit src;
        pname = "dassie";
        version = releaseVersion;
        strictDeps = true;
        cargoExtraArgs = "--locked -p dassie-cli";
        CARGO_PROFILE = profile;
        doCheck = false;
      };
    in
    crossPkgs.callPackage (
      { }:
      craneLib.buildPackage (
        common
        // {
          cargoArtifacts = craneLib.buildDepsOnly common;
          DASSIE_WALLET_DIST = wallet;
          DASSIE_VERSION = version;
        }
        // lib.optionalAttrs (key != null) { DASSIE_RELEASE_KEY = key; }
      )
    ) { };

  platforms = {
    x64 = {
      crossSystem = lib.systems.examples.musl64;
      target = "x86_64-unknown-linux-musl";
    };
    arm64 = {
      crossSystem = lib.systems.examples.aarch64-multiplatform-musl;
      target = "aarch64-unknown-linux-musl";
    };
  };

  binaries = version: lib.mapAttrs (_: platform: dassieFor (platform // { inherit version; })) platforms;

  # One platform's bundle, `dassie-<version>-linux-<arch>.tar.{gz,xz}`,
  # whose `dassie/` becomes /opt/dassie/<version>: the binary and the
  # systemd units. The archives are the same bytes for the same input:
  # sorted, with fixed times and owners.
  bundle = bundleWith ../resources/systemd;
  bundleWith =
    units: version: arch: dassie:
    pkgs.runCommand "dassie-${version}-linux-${arch}"
      {
        nativeBuildInputs = [
          pkgs.gnutar
          pkgs.gzip
          pkgs.xz
        ];
      }
      ''
        mkdir -p dassie/bin dassie/share/systemd
        install -m 755 ${dassie}/bin/dassie dassie/bin/dassie
        install -m 644 ${units}/* dassie/share/systemd/
        tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
          --format=gnu -cf bundle.tar dassie
        name=dassie-${version}-linux-${arch}
        mkdir $out
        gzip -9 --no-name < bundle.tar > $out/$name.tar.gz
        xz -6 --threads=1 < bundle.tar > $out/$name.tar.xz
      '';

  # install.sh with a key built in.
  installer =
    key:
    pkgs.runCommand "dassie-install.sh" { } ''
      substitute ${../resources/installer/install.sh} $out \
        --replace-fail @RELEASE_KEY@ "$(sed -n 2p ${key})"
      chmod 755 $out
    '';

  # What a release uploads to the mirror, but for the signature:
  #   <version>/dassie-<version>-linux-{x64,arm64}.tar.{gz,xz}
  #   <version>/SHA256SUMS
  #   meta/<channel>, which holds the version
  #   install.sh, once there is a release key
  # The release workflow signs SHA256SUMS (SHA256SUMS.minisig) and uploads
  # the tree.
  mirror =
    version: channel:
    let
      bundles = lib.mapAttrsToList (bundle version) (binaries version);
    in
    pkgs.runCommand "dassie-mirror-${version}" { } (
      ''
        mkdir -p $out/${version} $out/meta
        for bundle in ${lib.concatStringsSep " " bundles}; do
          cp $bundle/* $out/${version}/
        done
        (cd $out/${version} && sha256sum dassie-* > SHA256SUMS)
        printf '%s' ${version} > $out/meta/${channel}
      ''
      + lib.optionalString (releaseKey != null) ''
        cp ${installer releaseKey} $out/install.sh
      ''
    );

  # The releases the install tests serve (tests/e2e/install_test.ts): three
  # releases and a canary for x64, trusting the test key and signed with
  # it, beside install.sh with the test key. The node of 1.0.2 does not
  # start, so that the updater rolls it back. The tests write
  # meta/<channel>, and make their own tampered copies.
  brokenUnits = pkgs.runCommand "dassie-units-broken" { } ''
    cp -r ${../resources/systemd} $out
    chmod -R u+w $out
    substituteInPlace $out/dassie.service \
      --replace-fail "--runtime=systemd" "--runtime=systemd --no-such-option"
  '';
  testRelease =
    version: units:
    let
      dassie = dassieFor (
        platforms.x64
        // {
          inherit version;
          key = testKey;
          profile = "install-test";
        }
      );
    in
    pkgs.runCommand "dassie-test-release-${version}" { nativeBuildInputs = [ pkgs.minisign ]; } ''
      mkdir $out
      cp ${bundleWith units version "x64" dassie}/* $out/
      (cd $out && sha256sum dassie-* > SHA256SUMS)
      minisign -S -s ${../testdata/release/test.key} -m $out/SHA256SUMS \
        -x $out/SHA256SUMS.minisig -t "dassie ${version}"
    '';
  testCanary = "canary-2026-10-16-120000-abcdef0";
  install-test-mirror = pkgs.runCommand "dassie-install-test-mirror" { } ''
    mkdir $out
    cp -r ${testRelease "1.0.0" ../resources/systemd} $out/1.0.0
    cp -r ${testRelease "1.0.1" ../resources/systemd} $out/1.0.1
    cp -r ${testRelease "1.0.2" brokenUnits} $out/1.0.2
    cp -r ${testRelease testCanary ../resources/systemd} $out/${testCanary}
    cp ${installer testKey} $out/install.sh
  '';
  released = binaries releaseVersion;

  # The release binaries are static, name their version, and pass
  # `verify-install`, the arm64 one under qemu.
  static-binaries =
    pkgs.runCommand "dassie-static-binaries"
      {
        nativeBuildInputs = [
          pkgs.binutils-unwrapped
          pkgs.qemu-user
        ];
      }
      ''
        check() {
          local binary=$1 run=$2
          if readelf --program-headers $binary | grep -q INTERP \
            || readelf --dynamic $binary | grep -q NEEDED; then
            echo "$binary is linked dynamically" >&2
            exit 1
          fi
          [ "$($run $binary --version)" = "dassie ${releaseVersion}" ]
          $run $binary verify-install
        }
        check ${released.x64}/bin/dassie ""
        check ${released.arm64}/bin/dassie qemu-aarch64
        touch $out
      '';
in
{
  packages = {
    inherit wallet;
    dassie-x64 = released.x64;
    dassie-arm64 = released.arm64;
    release = mirror releaseVersion "latest";
    canary = mirror canaryVersion "canary";
    # Built by the release workflow, not `nix flake check`: what only a
    # release needs stays out of every PR's CI.
    inherit install-test-mirror static-binaries;
  };
  checks = {
    # The installer is POSIX sh, as ShellCheck reads it.
    installer = pkgs.runCommand "dassie-installer-shellcheck" { nativeBuildInputs = [ pkgs.shellcheck ]; } ''
      shellcheck --shell=sh ${../resources/installer/install.sh}
      touch $out
    '';
  };
}
