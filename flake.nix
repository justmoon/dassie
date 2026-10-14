{
  # Builds and checks for the Dassie rewrite (spec D14). Plain `cargo` and
  # `deno task` work locally as well; this flake pins the toolchain for CI
  # and, from M8, release builds.
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

          # The Rust workspace, with the golden vectors the tests read, the
          # RPC contract dassie-node generates its messages from, and the
          # test certificates two in-process daemons peer with.
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./clippy.toml
              ./crates
              ./proto
              ./testdata/pebble
            ];
          };

          common = {
            inherit src;
            pname = "dassie";
            version = "0.1.0";
            strictDeps = true;
            cargoExtraArgs = "--locked";
          };
          cargoArtifacts = craneLib.buildDepsOnly common;
          wasmArtifacts = craneLib.buildDepsOnly (
            common
            // {
              pname = "dassie-wasm";
              CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
              cargoExtraArgs = "--locked --features wasm -p dassie-identity -p dassie-ilp -p dassie-stream";
              doCheck = false;
            }
          );

          # The WASM modules and their bindings, as scripts/build-wasm.ts
          # makes them: deno/ for Deno, web/ for browsers.
          identity-wasm = craneLib.mkCargoDerivation (
            common
            // {
              pname = "identity-wasm";
              cargoArtifacts = wasmArtifacts;
              nativeBuildInputs = [ wasm-bindgen ];
              buildPhaseCargoCommand = ''
                for crate in dassie-identity dassie-stream; do
                  cargo rustc --locked -p $crate --lib --crate-type cdylib \
                    --target wasm32-unknown-unknown --profile wasm --features wasm
                done
              '';
              doInstallCargoArtifacts = false;
              installPhaseCommand = ''
                for module in dassie_identity dassie_stream; do
                  for target in deno web; do
                    wasm-bindgen --target $target --out-dir $out/$target \
                      target/wasm32-unknown-unknown/wasm/$module.wasm
                  done
                done
              '';
            }
          );
        in
        {
          packages = {
            inherit identity-wasm wasm-bindgen;
            default = identity-wasm;
          };

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
                CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
                cargoExtraArgs = "--locked --features wasm -p dassie-identity -p dassie-ilp -p dassie-stream";
                cargoClippyExtraArgs = "-- -D warnings";
              }
            );
            # systemd-socket-activate, for dassie-cli's process tests, which
            # skip without it.
            cargo-test = craneLib.cargoTest (
              common
              // {
                inherit cargoArtifacts;
                nativeCheckInputs = [ pkgs.systemd ];
              }
            );
            inherit identity-wasm;
          };

          devShells.default = pkgs.mkShell {
            packages = [
              toolchain
              wasm-bindgen
              pkgs.deno
              pkgs.chromium
              # The end-to-end tests (tests/e2e): Let's Encrypt's test ACME
              # server, and systemd's socket activation without systemd.
              pkgs.pebble
              pkgs.systemd
              # For regenerating the golden vectors (testdata/original).
              pkgs.nodejs
              pkgs.pnpm
            ];
            CHROME_BIN = lib.getExe pkgs.chromium;
          };
        };
    in
    {
      packages = lib.genAttrs systems (system: (perSystem system).packages);
      checks = lib.genAttrs systems (system: (perSystem system).checks);
      devShells = lib.genAttrs systems (system: (perSystem system).devShells);
    };
}
