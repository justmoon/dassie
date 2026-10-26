{
  # Builds and checks for the Dassie rewrite
  # (spec/decisions/build-system.md). Plain `cargo` and `node --run` work
  # locally as well; this flake pins the toolchain for CI and makes the
  # releases (nix/release.nix).
  description = "Dassie: a multi-currency, peer-to-peer, Interledger-enabled payment network";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      crane,
    }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      # The wasm-bindgen CLI, as release binaries pinned by hash. Its version
      # must be the wasm-bindgen crate's in Cargo.lock: the CLI and the crate
      # speak one schema, and evaluation fails if they differ.
      wasmBindgenVersion = "0.2.129";
      wasmBindgenHashes = {
        x86_64-linux = "sha256-gtEruUDi1OcuDVYFOH/BuMoXkETgErYg8M5OdEDoMg4=";
        aarch64-linux = "sha256-LtQ1HDXdlEAwi7sCdn1H6ieO/oUaUkZTAPPJT1tsKoc=";
      };

      perSystem =
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };

          # rust-toolchain.toml names only the channel; components and the
          # wasm target are added here, so that rustup never downloads them in
          # an offline build.
          toolchain = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
            extensions = [
              "clippy"
              "rustfmt"
            ];
            targets = [ "wasm32-unknown-unknown" ];
          };
          craneLib = (crane.mkLib pkgs).overrideToolchain (_: toolchain);

          lockedBindgen = lib.findFirst (p: p.name == "wasm-bindgen") null (
            lib.importTOML ./Cargo.lock
          ).package;
          wasm-bindgen =
            assert lib.assertMsg (lockedBindgen.version == wasmBindgenVersion)
              "wasm-bindgen: the CLI (flake.nix) is ${wasmBindgenVersion}, Cargo.lock has ${lockedBindgen.version}; they must match";
            pkgs.stdenvNoCC.mkDerivation {
              pname = "wasm-bindgen-cli";
              version = wasmBindgenVersion;
              src = pkgs.fetchurl {
                url =
                  let
                    arch = lib.head (lib.splitString "-" system);
                  in
                  "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${wasmBindgenVersion}/wasm-bindgen-${wasmBindgenVersion}-${arch}-unknown-linux-musl.tar.gz";
                hash = wasmBindgenHashes.${system};
              };
              installPhase = "install -Dm755 wasm-bindgen $out/bin/wasm-bindgen";
            };

          # The Rust workspace, with Cargo's configuration (the property
          # tests' fixed seed, and the bundled SQLite's build flags, which
          # the release builds with too), the golden vectors the tests read (the rate
          # feed's are in testdata/feed), the RPC contract dassie-node
          # generates its messages from, the test certificates two
          # in-process daemons peer with, minisign's signatures with the
          # release test key, and the release's resources: its key, the
          # systemd units and install.sh. Of the proof servers'
          # files only their xrpld configs, whose history a test holds
          # above the account's history bound, so that changing the rest
          # rebuilds nothing.
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./.cargo/config.toml
              ./Cargo.toml
              ./Cargo.lock
              ./clippy.toml
              ./crates
              ./proto
              (lib.fileset.difference ./resources ./resources/proof-server)
              ./resources/proof-server/xrpld-mainnet.cfg
              ./resources/proof-server/xrpld-testnet.cfg
              ./testdata/feed
              ./testdata/tls
              ./testdata/release
            ];
          };

          # The desktop app's libraries (crates/dassie-desktop): GTK and
          # WebKitGTK for its window, libsoup for WebKit's network, and the
          # tray icon's AppIndicator. Every build of the whole workspace
          # compiles their -sys crates, so they are in `common`.
          desktop = import ./nix/desktop-libs.nix { inherit pkgs; };

          # The checks build with the ci profile (Cargo.toml), as a local
          # `cargo test` does. crane's default, release, gives every test
          # binary fat LTO and one codegen unit, which took the tests minutes
          # to link. The release builds use release (nix/release.nix).
          common = {
            inherit src;
            pname = "dassie";
            version = "0.1.0";
            strictDeps = true;
            cargoExtraArgs = "--locked";
            CARGO_PROFILE = "ci";
            inherit (desktop) nativeBuildInputs buildInputs;
          };
          cargoArtifacts = craneLib.buildDepsOnly common;
          wasmArtifacts = craneLib.buildDepsOnly (
            common
            // {
              pname = "dassie-wasm";
              # The profile wasm-modules builds with, so that it reuses
              # these dependencies rather than building them again.
              CARGO_PROFILE = "wasm";
              CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
              cargoExtraArgs = "--locked --features identity,stream -p dassie-wasm";
              doCheck = false;
            }
          );

          # The WASM modules and their bindings, as scripts/build-wasm.ts
          # makes them: web/, which browsers, scripts and tests all load.
          wasm-modules = craneLib.mkCargoDerivation (
            common
            // {
              pname = "wasm-modules";
              cargoArtifacts = wasmArtifacts;
              nativeBuildInputs = [ wasm-bindgen ];
              # Each of dassie-wasm's features is a module of its own; each
              # build writes the same file, so it is bound before the next.
              buildPhaseCargoCommand = ''
                for feature in identity stream; do
                  cargo rustc --locked -p dassie-wasm --lib --crate-type cdylib \
                    --target wasm32-unknown-unknown --profile wasm --features $feature
                  wasm-bindgen --target web --out-dir wasm-out/web --out-name dassie_$feature \
                    target/wasm32-unknown-unknown/wasm/dassie_wasm.wasm
                done
              '';
              doInstallCargoArtifacts = false;
              installPhaseCommand = ''
                mkdir -p $out
                cp -r wasm-out/web $out/web
              '';
            }
          );

          # `dassie-release verify`, with which the release workflow checks
          # what it signed.
          release-tool = craneLib.buildPackage (
            common
            // {
              pname = "dassie-release";
              inherit cargoArtifacts;
              cargoExtraArgs = "--locked -p dassie-release --features tool";
              doCheck = false;
            }
          );

          # The release builds (nix/release.nix), made on x86_64-linux only.
          release = lib.optionalAttrs (system == "x86_64-linux") (
            import ./nix/release.nix {
              inherit
                lib
                pkgs
                nixpkgs
                rust-overlay
                crane
                src
                wasm-modules
                ;
            }
          );

          # The desktop app (nix/desktop.nix), with the wallet built in, on
          # x86_64-linux, where the wallet is built: `nix run .#desktop`.
          # The package builds in release; its check, below, in
          # desktop-check.
          desktopIn =
            profile:
            import ./nix/desktop.nix {
              inherit
                lib
                pkgs
                craneLib
                src
                profile
                ;
              inherit (release.packages) wallet;
            };
          desktop-app = lib.optionalAttrs (system == "x86_64-linux") {
            desktop = desktopIn "release";
          };
          desktop-check = lib.optionalAttrs (system == "x86_64-linux") {
            desktop = desktopIn "desktop-check";
          };

          # The TypeScript side's checks (nix/ts.nix), on x86_64-linux too,
          # where the wallet they test is built.
          ts = lib.optionalAttrs (system == "x86_64-linux") (
            import ./nix/ts.nix {
              inherit
                lib
                pkgs
                craneLib
                common
                toolchain
                wasm-modules
                ;
              inherit (release.packages) wallet;
            }
          );
        in
        {
          packages = {
            inherit wasm-modules wasm-bindgen release-tool;
          }
          # `nix build` alone makes the static `dassie` with the wallet
          # built in, where the release builds are made.
          // lib.optionalAttrs (release ? packages) { default = release.packages.dassie-x64; }
          // (release.packages or { })
          // (ts.packages or { })
          // desktop-app;

          checks = {
            cargo-fmt = craneLib.cargoFmt {
              inherit src;
              inherit (common) pname version;
            };
            cargo-clippy = craneLib.cargoClippy (
              common
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- -D warnings";
              }
            );
            cargo-clippy-wasm = craneLib.cargoClippy (
              common
              // {
                cargoArtifacts = wasmArtifacts;
                CARGO_PROFILE = "wasm";
                CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
                cargoExtraArgs = "--locked --features identity,stream -p dassie-wasm";
                cargoClippyExtraArgs = "-- -D warnings";
              }
            );
            # The workspace's tests, with cargo-nextest: each test in its own
            # process and all at once, so that the check takes its build and
            # its slowest test, not every test binary one after another. Its
            # ci profile (.config/nextest.toml, which joins this check's
            # source alone) runs every test, retries none and stops a hang.
            # nextest runs no doc tests, so `cargo test --doc` runs them
            # after. systemd-socket-activate, for dassie-cli's process tests,
            # which skip without it. The test key, for its update tests.
            # Nothing builds on the check, so it keeps no target directory.
            cargo-test = craneLib.cargoNextest (
              common
              // {
                src = lib.fileset.toSource {
                  root = ./.;
                  fileset = lib.fileset.unions [
                    (lib.fileset.fromSource src)
                    ./.config/nextest.toml
                  ];
                };
                inherit cargoArtifacts;
                nativeCheckInputs = [ pkgs.systemd ];
                NEXTEST_PROFILE = "ci";
                cargoNextestExtraArgs = "--features dassie-cli/test-key";
                postCheck = ''
                  cargoWithProfile test --doc --locked --features dassie-cli/test-key
                '';
                doInstallCargoArtifacts = false;
              }
            );
            # The dependencies' licenses and sources, and wildcard
            # requirements, by deny.toml: crane's default checks, which
            # need no network. The advisories change without a change
            # here, so they run in their own workflow (advisories.yml).
            # deny.toml joins the source of this check alone, so that a
            # change to it builds nothing else again.
            cargo-deny = craneLib.cargoDeny {
              src = lib.fileset.toSource {
                root = ./.;
                fileset = lib.fileset.unions [
                  (lib.fileset.fromSource src)
                  ./deny.toml
                ];
              };
              inherit (common) pname version;
            };
            inherit wasm-modules;
          }
          // (release.checks or { })
          // (ts.checks or { })
          # The packaged app builds and is wrapped, which clippy and the
          # tests do not show. It builds the package's code in the release
          # profile's settings but for fat LTO (desktop-check, Cargo.toml):
          # optimizing the whole app as one unit took it minutes on every
          # change to the node, and how well the app is optimized is not
          # what it checks.
          // desktop-check;

          # The simulation campaign's VMs (scripts/sim-campaign.ts): CI's
          # toolchain, and Node for the supervisor, without the rest of the
          # default shell for every VM to download.
          devShells.campaign = pkgs.mkShell {
            packages = [
              toolchain
              pkgs.nodejs_26
            ];
          };

          devShells.default = pkgs.mkShell {
            packages = [
              toolchain
              wasm-bindgen
              # The TypeScript side: Node strips the types, pnpm installs the
              # workspace (pnpm-lock.yaml). pnpm 11, as pnpm_11: nixpkgs'
              # pnpm is 12, an experimental rewrite in Rust that can't
              # install from a read-only store. The same two regenerate the
              # golden vectors (testdata/original): the original names pnpm
              # 9, which pnpm 11 fetches and hands the install to, and
              # wants Node 22, but its Vitest makes the same vectors with 26.
              pkgs.nodejs_26
              pkgs.pnpm_11
              # The browser tests (CHROME_BIN).
              pkgs.chromium
              # The end-to-end tests (tests/e2e): Let's Encrypt's test ACME
              # server, and systemd's socket activation without systemd.
              pkgs.pebble
              pkgs.systemd
              # The dev environment's and the scenarios' certificates
              # (apps/dev).
              pkgs.mkcert
              # Signing releases (scripts/release.ts).
              pkgs.minisign
            ]
            # The desktop app (crates/dassie-desktop).
            ++ desktop.nativeBuildInputs
            ++ desktop.buildInputs;
            CHROME_BIN = lib.getExe pkgs.chromium;
            # Node keeps what it compiles between runs, which starts the
            # tests, scripts and the dev environment faster. It does only
            # with a directory to keep it in: the user's cache.
            shellHook = ''
              export NODE_COMPILE_CACHE="''${XDG_CACHE_HOME:-$HOME/.cache}/node-compile-cache"
            '';
          };
        };
    in
    {
      packages = lib.genAttrs systems (system: (perSystem system).packages);
      checks = lib.genAttrs systems (system: (perSystem system).checks);
      devShells = lib.genAttrs systems (system: (perSystem system).devShells);
    };
}
