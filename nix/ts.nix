# The TypeScript side's checks (spec/decisions/build-system.md): what
# `node --run ci` runs, in the build sandbox, offline. The npm and JSR
# packages come from the lockfile (nix/pnpm.nix), dprint's plugins from the
# checksums dprint.json pins them with, and the binaries the tests run
# (`e2e`) crate by crate (nix/rust), where `node --run build:e2e` puts them.
#
# Four checks, so that a change reruns only what reads it:
# - ts-check: fmt, lint, the type check and `buf lint` of the contracts
#   in proto/;
# - ts-test: the unit tests, with the wallet, the dashboard and the site
#   built;
# - spec: the spec's links and citations, the only test that reads the
#   docs;
# - ts-e2e: the end-to-end and interop tests, with Chrome, Pebble and
#   systemd's socket activation. Each build has its own network namespace,
#   so the tests' fixed ports are free.
#
# What the sandbox can't do, checking the lockfile against the registries
# (the release-age rule), CI's `nix` job does on the runner.
{
  lib,
  pkgs,
  craneLib,
  common,
  toolchain,
  wasm-modules,
  wallet,
  e2e,
}:
let
  pnpm = import ./pnpm.nix { inherit lib pkgs; };

  # The repository as checked out, but for `exclude`, which a check doesn't
  # read: a change there leaves its result as it is.
  tree =
    exclude:
    lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.difference ../. (lib.fileset.unions exclude);
    };
  # What only the spec's test reads, and CI's workflows, which only the
  # site's test reads.
  docs = [
    ../spec
    ../AGENTS.md
    ../CLAUDE.md
    ../README.md
  ];
  workflows = [ ../.github ];

  # dprint.json with each plugin's URL replaced by the plugin, fetched by
  # the checksum the URL ends in. The file keeps its layout, so that dprint
  # finds it formatted.
  dprintJson = builtins.readFile ../dprint.json;
  dprintPlugins = (builtins.fromJSON dprintJson).plugins;
  dprintPlugin =
    plugin:
    let
      m = builtins.match "(https://.*)@([0-9a-f]{64})" plugin;
    in
    assert lib.assertMsg (m != null) "dprint.json: ${plugin} has no checksum (<url>@<sha256>)";
    pkgs.fetchurl {
      url = lib.elemAt m 0;
      sha256 = lib.elemAt m 1;
    };
  dprintOffline = pkgs.writeText "dprint.json" (
    builtins.replaceStrings dprintPlugins (map (p: "${dprintPlugin p}") dprintPlugins) dprintJson
  );

  # dassie.land (spec/areas/website.md), as `node --run build:website`
  # builds it, offline: the packages of the site and the members it uses,
  # each fetched as pnpm-lock.yaml pins it (nix/pnpm.nix), the WASM
  # bindings, and the release key the NixOS guide prints. What the website
  # workflow publishes, and what the checks below test.
  website = pkgs.stdenvNoCC.mkDerivation {
    name = "dassie-website";
    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        pnpm.manifests
        ../tsconfig.base.json
        ../apps/website
        ../packages/wasm
        ../packages/rpc
        ../packages/ui
        ../resources/minisign.pub
      ];
    };
    nativeBuildInputs = [
      pkgs.nodejs_26
      pkgs.pnpm_11
    ];
    buildPhase = ''
      export HOME=$TMPDIR
      mkdir -p packages/wasm/gen
      cp -r ${wasm-modules}/web packages/wasm/gen/
      chmod -R u+w packages/wasm/gen
    ''
    + pnpm.install "apps/website"
    + ''
      cd apps/website
      node --run build
    '';
    installPhase = "cp -r dist $out";
  };

  # Cargo's sources, for `cargo metadata`, which the site's and the
  # architecture page's tests read the workspace with.
  cargoVendor = craneLib.vendorCargoDeps { inherit (common) src; };

  # A check: the workspace installed (`dir`'s packages only, if named) and
  # the WASM bindings in place, the wallet, the site and the binaries too
  # with `binaries`, then `script`, with CI set, as GitHub's runners set it,
  # so that a test without its tools fails rather than skips.
  check =
    name:
    {
      exclude ? [ ],
      dir ? null,
      binaries ? false,
      inputs ? [ ],
      script,
    }:
    pkgs.stdenv.mkDerivation {
      name = "dassie-${name}";
      src = tree exclude;
      nativeBuildInputs = [
        pkgs.nodejs_26
        pkgs.pnpm_11
        pkgs.which
        pkgs.autoPatchelfHook
      ]
      ++ inputs;
      # dprint's binary, from npm, links against glibc and libgcc.
      buildInputs = [ pkgs.stdenv.cc.cc.lib ];
      dontConfigure = true;
      dontFixup = true;
      buildPhase = ''
        export HOME=$TMPDIR
        mkdir -p packages/wasm/gen
        cp -r ${wasm-modules}/web packages/wasm/gen/
        chmod -R u+w packages/wasm/gen
      ''
      + pnpm.install dir
      + ''
        autoPatchelf node_modules/.pnpm/@dprint+linux-x64-glibc@*
      ''
      + lib.optionalString binaries ''
        mkdir -p apps/wallet/dist apps/website/dist target/e2e
        cp -r ${wallet}/. apps/wallet/dist/
        cp -r ${website}/. apps/website/dist/
        cp -r ${e2e}/. target/e2e/
        chmod -R u+w apps/wallet/dist apps/website/dist target/e2e
      ''
      + ''
        export CI=true
      ''
      + script;
      installPhase = "touch $out";
    };
in
{
  packages = {
    e2e-binaries = e2e;
    inherit website;
  };

  checks = {
    ts-check = check "ts-check" {
      exclude = docs ++ workflows;
      script = ''
        cp ${dprintOffline} dprint.json
        node --run check
      '';
    };

    # The spec's test leaves for the spec check, which reads the docs this
    # one leaves out.
    ts-test = check "ts-test" {
      exclude = docs;
      binaries = true;
      inputs = [
        toolchain
        pkgs.minisign
        pkgs.mkcert
      ];
      script = ''
        export CARGO_HOME=$TMPDIR/cargo
        mkdir -p $CARGO_HOME
        cp ${cargoVendor}/config.toml $CARGO_HOME/config.toml
        rm scripts/spec_test.ts
        node --run build:dashboard
        node --run test
      '';
    };

    # Every link and citation of the spec, against the whole tree.
    spec = check "spec" {
      dir = ".";
      script = ''
        node --test scripts/spec_test.ts
      '';
    };

    ts-e2e = check "ts-e2e" {
      exclude = docs ++ workflows;
      binaries = true;
      inputs = [
        pkgs.chromium
        pkgs.pebble
        pkgs.systemd
        pkgs.mkcert
      ];
      script = ''
        export CHROME_BIN=${lib.getExe pkgs.chromium}
        export FONTCONFIG_FILE=${pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; }}
        export DASSIE_INTEROP_PEER=$PWD/target/e2e/examples/interop_peer
        node --run test:e2e
      '';
    };
  };
}
