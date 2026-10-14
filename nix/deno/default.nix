# The Deno side of `deno task ci`, as derivations: the wallet and dashboard
# builds, and checks that each see only the sources they need, so that
# nixbuild.net runs them in parallel and reruns only those whose inputs
# changed.
{
  pkgs,
  deno,
  # The WASM bindings (packages/identity-wasm/gen), as scripts/build-wasm.ts
  # makes them.
  identity-wasm,
  # The binary the end-to-end tests run (target/e2e/dassie).
  dassie-e2e,
  # crates/dassie-stream's interop_peer example (tests/interop).
  interop-peer,
}:
let
  inherit (pkgs) lib;
  root = ../..;
  deps = import ./deps.nix { inherit pkgs deno; };

  # The workspace members, and the members each imports.
  members = {
    identity-wasm = {
      dir = "packages/identity-wasm";
      uses = [ ];
    };
    rpc = {
      dir = "packages/rpc";
      uses = [ ];
    };
    ui = {
      dir = "packages/ui";
      uses = [
        "identity-wasm"
        "rpc"
      ];
    };
    wallet = {
      dir = "apps/wallet";
      uses = [
        "identity-wasm"
        "rpc"
        "ui"
      ];
    };
    dev = {
      dir = "apps/dev";
      uses = [
        "identity-wasm"
        "rpc"
        "ui"
      ];
    };
    dev-dashboard = {
      dir = "apps/dev-dashboard";
      uses = [
        "rpc"
        "ui"
      ];
    };
    e2e = {
      dir = "tests/e2e";
      uses = [
        "dev"
        "identity-wasm"
        "rpc"
        "ui"
      ];
    };
    interop = {
      dir = "tests/interop";
      uses = [ "identity-wasm" ];
    };
  };

  # The manifests, these members and those they import, and other files.
  filesetFor =
    names: extra:
    let
      closure = lib.unique (lib.concatMap (n: [ n ] ++ members.${n}.uses) names);
    in
    lib.fileset.unions (deps.manifests ++ map (n: root + "/${members.${n}.dir}") closure ++ extra);
  srcFor =
    names: extra:
    lib.fileset.toSource {
      inherit root;
      fileset = filesetFor names extra;
    };

  # The installed dependencies and the WASM bindings, in place.
  setup = ''
    cp -a ${deps.installed}/node_modules node_modules
    cp -a ${deps.installed}/deno-dir $TMPDIR/deno-dir
    chmod -R u+w node_modules $TMPDIR/deno-dir
    export DENO_DIR=$TMPDIR/deno-dir HOME=$TMPDIR DENO_NO_UPDATE_CHECK=1
    mkdir -p packages/identity-wasm/gen
    cp -r ${identity-wasm}/. packages/identity-wasm/gen/
    chmod -R u+w packages/identity-wasm/gen
  '';

  # `deno task build` in an app: its dist/.
  viteBuild =
    name:
    pkgs.stdenvNoCC.mkDerivation {
      inherit name;
      src = srcFor [ name ] [ ];
      nativeBuildInputs = [ deno ];
      buildPhase =
        setup
        + deps.withMirror ''
          (cd ${members.${name}.dir} && deno task build)
        '';
      installPhase = "cp -r ${members.${name}.dir}/dist $out";
      dontFixup = true;
    };

  wallet = viteBuild "wallet";
  dev-dashboard = viteBuild "dev-dashboard";

  # `deno task test` for some of the tests. Types are checked once, by
  # deno-check.
  denoTest =
    {
      name,
      uses,
      tests,
      extra ? [ ],
      e2e ? false,
      # Chrome, for astral (e2e implies it).
      browser ? e2e,
      env ? { },
    }:
    pkgs.stdenvNoCC.mkDerivation (
      env
      // lib.optionalAttrs browser {
        CHROME_BIN = lib.getExe pkgs.chromium;
        # Without fonts, Chrome logs an error for every glyph it shapes.
        # astral stops reading Chrome's stderr once it has started, so the
        # pipe fills and Chrome stops in the middle of loading a page.
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      }
      // {
        name = "deno-${name}";
        # The interop and end-to-end test files other than these are left
        # out, so that a change to one reruns only its check.
        src = lib.fileset.toSource {
          inherit root;
          fileset = lib.fileset.difference (filesetFor uses extra) (
            lib.fileset.difference (lib.fileset.fileFilter (f: lib.hasSuffix "_test.ts" f.name) (
              root + "/tests"
            )) (lib.fileset.unions (map (t: lib.fileset.maybeMissing (root + "/${t}")) tests))
          );
        };
        nativeBuildInputs = [
          deno
          pkgs.which
        ]
        ++ lib.optional browser pkgs.chromium
        ++ lib.optionals e2e [
          pkgs.pebble
          pkgs.systemd
          pkgs.mkcert
        ];
        # The apps' builds where `deno task build:*` puts them, for their
        # build_test.ts.
        buildPhase =
          setup
          + lib.optionalString (lib.elem "wallet" uses) "ln -s ${wallet} apps/wallet/dist\n"
          + lib.optionalString (lib.elem "dev-dashboard" uses) "ln -s ${dev-dashboard} apps/dev-dashboard/dist\n"
          + lib.optionalString e2e ''
            mkdir -p target/e2e
            ln -s ${dassie-e2e}/bin/dassie target/e2e/dassie
            # A missing tool fails the tests rather than skipping them.
            export CI=1
          ''
          + deps.withMirror ''
            timeout 15m deno test --frozen --no-check --allow-read --allow-write --allow-env \
              --allow-net --allow-run --allow-sys=hostname ${lib.escapeShellArgs tests}
          '';
        installPhase = "touch $out";
        dontFixup = true;
      }
    );

  # Everything `deno fmt` and `deno check` read (deno.json's fmt.include).
  allSources = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions (
      deps.manifests
      ++ [
        (root + "/apps")
        (root + "/packages")
        (root + "/tests")
        (root + "/scripts")
      ]
    );
  };

  e2eTest =
    file: extraUses:
    denoTest {
      name = "e2e-" + lib.replaceStrings [ "_" ] [ "-" ] (lib.removeSuffix "_test.ts" file);
      uses = [ "e2e" ] ++ extraUses;
      tests = [ "tests/e2e/${file}" ];
      extra = [ (root + "/testdata/pebble") ];
      e2e = true;
    };

  interopTest =
    name: file: extra: env:
    denoTest {
      name = "interop-${name}";
      uses = [ "interop" ];
      tests = [ "tests/interop/${file}" ];
      inherit extra env;
    };

  unitTest =
    name: extra:
    denoTest {
      name = "test-${name}";
      uses = [ name ];
      tests = [ members.${name}.dir ];
      inherit extra;
    };
in
{
  inherit deps wallet dev-dashboard;

  checks = {
    deno-fmt-lint = pkgs.stdenvNoCC.mkDerivation {
      name = "deno-fmt-lint";
      src = allSources;
      nativeBuildInputs = [ deno ];
      buildPhase = ''
        export DENO_DIR=$TMPDIR/deno-dir HOME=$TMPDIR DENO_NO_UPDATE_CHECK=1
        deno fmt --check
        deno lint
      '';
      installPhase = "touch $out";
    };

    deno-check = pkgs.stdenvNoCC.mkDerivation {
      name = "deno-check";
      src = allSources;
      nativeBuildInputs = [ deno ];
      buildPhase = setup + deps.withMirror "deno check --frozen";
      installPhase = "touch $out";
    };

    # gen_test hashes the contracts the generated code came from.
    deno-test-rpc = unitTest "rpc" [ (root + "/proto") ];
    deno-test-ui = unitTest "ui" [ ];
    deno-test-wallet = unitTest "wallet" [ ];
    deno-test-dev-dashboard = unitTest "dev-dashboard" [ ];
    deno-test-dev = unitTest "dev" [ ];

    # The WASM bindings against the native crates' golden vectors.
    deno-interop-identity-wasm = interopTest "identity-wasm" "identity_wasm_test.ts" [
      (root + "/crates/dassie-identity/tests/data")
      (root + "/crates/dassie-ilp/tests/data")
    ] { };
    deno-interop-stream = interopTest "stream" "stream_interop_test.ts" [ ] { };
    deno-interop-native = interopTest "native" "native_interop_test.ts" [ ] {
      DASSIE_INTEROP_PEER = "${interop-peer}/bin/interop_peer";
    };
    deno-interop-browser = denoTest {
      name = "interop-browser";
      uses = [ "interop" ];
      tests = [ "tests/interop/browser_test.ts" ];
      browser = true;
    };

    deno-e2e-btp-payment = e2eTest "btp_payment_test.ts" [ ];
    # `deno task dev` builds the wallet and dashboard itself.
    deno-e2e-dev = e2eTest "dev_test.ts" [
      "wallet"
      "dev-dashboard"
    ];
    deno-e2e-fresh-install = e2eTest "fresh_install_test.ts" [ ];
    deno-e2e-open-payments = e2eTest "open_payments_test.ts" [ ];
    deno-e2e-scenarios = e2eTest "scenarios_test.ts" [ ];
    deno-e2e-third-party = e2eTest "third_party_test.ts" [ ];
    deno-e2e-wallet = e2eTest "wallet_test.ts" [ ];
  };
}
