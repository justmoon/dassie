# The Cargo workspace (crates/) as one derivation per crate, so that an
# edit rebuilds the crate edited and the crates built on it, and runs only
# their tests again; a lockfile bump rebuilds the dependencies it moved and
# what is built on them (spec/decisions/build-system.md). Cargo.nix is
# crate2nix's description of the workspace, committed and held to Cargo.lock
# by a check (`nix run .#cargo-nix` after a change to a manifest or the
# lockfile); nixpkgs' buildRustCrate builds it.
#
# For each member, as checks: its tests, run with nextest (crate-<name>-test),
# and clippy over its library and binaries (crate-<name>-clippy) and over its
# tests and examples (crate-<name>-clippy-tests).
{
  pkgs,
  toolchain,
  craneLib,
}:
let
  inherit (pkgs) lib;
  root = ../..;
  workspace = (lib.importTOML (root + "/Cargo.toml")).workspace;
  # Every member but the desktop app, whose Tauri dependencies' build
  # scripts hand their dependents metadata under names with a colon in them,
  # which buildRustCrate passes on as shell variables and bash refuses: the
  # flake checks it with Cargo (desktop-test).
  members = lib.remove "dassie-desktop" (map (lib.removePrefix "crates/") workspace.members);

  # What a member reads from outside its directory: its library, binaries
  # and build script (extraFiles), and its tests and examples too
  # (testExtraFiles). A new read outside the crate needs an entry here;
  # without one the build fails for want of the file.
  extraFiles = {
    dassie-cli = [
      (root + "/resources")
      (root + "/testdata/release")
      (root + "/testdata/tls")
    ];
    # The RPC contract its build script generates its messages from.
    dassie-node = [
      (root + "/proto")
      (root + "/testdata/tls")
    ];
    dassie-release = [ (root + "/testdata/release") ];
  };
  testExtraFiles = {
    dassie-feed = [ (root + "/testdata/feed") ];
    dassie-hub = [ (root + "/testdata/hub") ];
    dassie-peer = [ (root + "/crates/dassie-oer/tests/data") ];
    dassie-xrpl = [
      (root + "/crates/dassie-identity/tests/data/chain.json")
      (root + "/resources/proof-server/xrpld-mainnet.cfg")
      (root + "/resources/proof-server/xrpld-testnet.cfg")
    ];
  };
  # Features a member's tests build with beyond its default and those its
  # dev-dependencies turn on: what CI's `cargo test` named.
  testFeatures = {
    dassie-cli = [ "test-key" ];
  };

  # What only a member's tests and examples read: an edit there runs its
  # own tests again, not everything built on it.
  testOnly =
    name:
    let
      dir = root + "/crates/${name}";
    in
    lib.fileset.unions (
      map (d: lib.fileset.maybeMissing (dir + "/${d}")) [
        "tests"
        "benches"
        "examples"
      ]
    );
  memberSrc =
    name: tests:
    let
      dir = root + "/crates/${name}";
    in
    lib.fileset.toSource {
      inherit root;
      fileset = lib.fileset.unions (
        [ (if tests then dir else lib.fileset.difference dir (testOnly name)) ]
        ++ extraFiles.${name} or [ ]
        ++ lib.optionals tests (testExtraFiles.${name} or [ ])
      );
    };
  targetSrc =
    name: fileset:
    lib.fileset.toSource {
      inherit root;
      fileset = lib.fileset.unions (
        [ fileset ] ++ extraFiles.${name} or [ ] ++ testExtraFiles.${name} or [ ]
      );
    };

  # The ci profile (Cargo.toml) for every crate, dependencies included, as
  # Cargo applies a profile: line tables, and overflow checks and debug
  # assertions on, since a test is to check every invariant
  # (spec/decisions/tigerstyle.md). Dependencies are optimized, as crypto is
  # unbearably slow without; the workspace's crates compile at opt-level 1,
  # in as many codegen units as Cargo's: buildRustCrate's default is one,
  # on one core, and optimizing them further bought the tests nothing.
  ciFlags = [
    "-C"
    "debuginfo=line-tables-only"
    "-C"
    "overflow-checks=on"
    "-C"
    "debug-assertions=on"
  ];
  # Cargo compiles from the workspace's root, so a panic, `file!()` and a
  # test's failure name `crates/<name>/src/...`, as the simulation
  # campaign's failure signatures do; buildRustCrate compiles from the
  # crate's directory, and the paths are remapped to Cargo's.
  memberFlags = name: crate: {
    codegenUnits = 16;
    extraRustcOpts =
      (crate.extraRustcOpts or [ ])
      ++ [
        "-C"
        "opt-level=1"
      ]
      ++ map (dir: "--remap-path-prefix=${dir}=crates/${name}/${dir}") [
        "src"
        "tests"
        "examples"
        "build.rs"
      ];
  };

  # The build scripts' and the tests' environment, as .cargo/config.toml
  # sets it for Cargo: the bundled SQLite's flags, which the release builds
  # with too, and the property tests' fixed seed. A test's scratch space,
  # which `env!("CARGO_TARGET_TMPDIR")` names when it compiles, is made
  # where its tests run (runTests).
  targetTmpDir = "/build/target-tmp";

  crateOverrides =
    pkgs.defaultCrateOverrides
    // {
      libsqlite3-sys = attrs: {
        LIBSQLITE3_FLAGS = "-DSQLITE_DEFAULT_MEMSTATUS=0";
      };
    }
    // lib.genAttrs members (
      name: crate:
      {
        # A target of a split test build (splitTests) brings its own.
        src = crate.targetSrc or (memberSrc name (crate.buildTests or false));
        sourceRoot = "source/crates/${name}";
        # Cargo sets these and buildRustCrate doesn't.
        CARGO_CRATE_NAME = lib.replaceStrings [ "-" ] [ "_" ] name;
        CARGO = "${toolchain}/bin/cargo";
        CARGO_TARGET_TMPDIR = targetTmpDir;
        lints = workspace.lints;
      }
      // memberFlags name crate
    );

  buildRustCrateForPkgs =
    p: crate:
    (p.buildRustCrate.override {
      rustc = toolchain;
      cargo = toolchain;
      defaultCrateOverrides = crateOverrides;
    })
      (
        crate
        // {
          extraRustcOpts = (crate.extraRustcOpts or [ ]) ++ ciFlags;
        }
      );

  # clippy-driver in place of rustc for the workspace's crates, dependencies
  # staying the plain builds every other derivation shares, with the
  # workspace's lints (Cargo.toml) and its clippy.toml, or a crate's own.
  # Lints are capped at `allow` for dependencies; `forbid` is no cap, where
  # `warn` would turn `-D warnings` back into warnings.
  clippyConfig = lib.fileset.toSource {
    inherit root;
    fileset = root + "/clippy.toml";
  };
  clippyConfigOf =
    name:
    let
      own = root + "/crates/${name}/clippy.toml";
    in
    if builtins.pathExists own then
      "${
        lib.fileset.toSource {
          inherit root;
          fileset = own;
        }
      }/crates/${name}"
    else
      "${clippyConfig}";
  # buildRustCrate compiles a build script with plain `rustc` whatever
  # useClippy says: this `rustc`, ahead of the toolchain's on the path,
  # hands a build script to clippy-driver, so that build.rs is linted as
  # `cargo clippy` lints it.
  clippyBuildScripts = pkgs.writeShellScriptBin "rustc" ''
    case " $* " in
      *" --crate-name build_script_build "*) exec ${toolchain}/bin/clippy-driver "$@" ;;
    esac
    exec ${toolchain}/bin/rustc "$@"
  '';
  buildWithClippy =
    p: crate:
    let
      member = crate.memberName or crate.crateName;
      denyWarnings = [
        "-D"
        "warnings"
      ];
    in
    buildRustCrateForPkgs p (
      crate
      // lib.optionalAttrs (lib.elem member members) {
        useClippy = true;
        clippy = toolchain;
        capLints = "forbid";
        CLIPPY_CONF_DIR = clippyConfigOf member;
        extraRustcOpts = (crate.extraRustcOpts or [ ]) ++ denyWarnings;
        extraRustcOptsForBuildRs = (crate.extraRustcOptsForBuildRs or [ ]) ++ denyWarnings;
        preConfigure = (crate.preConfigure or "") + ''
          export PATH=${clippyBuildScripts}/bin:$PATH
        '';
      }
    );

  cargoNix = import ./Cargo.nix {
    inherit pkgs buildRustCrateForPkgs;
  };

  # The features a member turns on for its own tests by depending on itself
  # (`[dev-dependencies] dassie-node = { path = ".", features = [...] }`),
  # which Cargo unifies into the crate its tests build. buildRustCrate
  # builds a crate and its tests apart, so its tests build with those
  # features, and Cargo.nix's crates leave its dependency on itself out.
  selfTestFeatures =
    name:
    ((lib.importTOML (root + "/crates/${name}/Cargo.toml")).dev-dependencies or { }).${name}.features
      or [ ];
  withoutSelf =
    name:
    cargoNix.internal.crates
    // {
      ${name} = cargoNix.internal.crates.${name} // {
        devDependencies = lib.filter (dep: dep.packageId != name) (
          cargoNix.internal.crates.${name}.devDependencies or [ ]
        );
      };
    };

  # The .rs files directly in a member's `dir` (tests/ or examples/).
  targetsIn =
    name: dir:
    let
      path = root + "/crates/${name}/${dir}";
    in
    if builtins.pathExists path then
      map (lib.removeSuffix ".rs") (
        lib.attrNames (
          lib.filterAttrs (file: type: type == "regular" && lib.hasSuffix ".rs" file) (builtins.readDir path)
        )
      )
    else
      [ ];

  # A member's tests, each target in a derivation of its own, so that they
  # compile at once and an edit to one test file compiles that one again:
  # buildRustCrate would compile the library, its unit tests and every file
  # in tests/ one after another. The library builds on its own, with the
  # features its tests turn on, and its binaries, which integration tests
  # run as CARGO_BIN_EXE_<name>; its unit tests from its source without
  # tests/ or examples/; and each file in tests/, and each example, from
  # the crate without src/, with what the files beside it share
  # (tests/support), linking that library.
  splitTests =
    name: build: p: crate:
    let
      dir = root + "/crates/${name}";
      library = build p (crate // { buildTests = false; });
      unit = build p (
        crate
        // {
          buildTests = true;
          # Unit tests may read the test data beside tests/ (tests/data).
          targetSrc = targetSrc name (lib.fileset.union (lib.fileset.difference dir (testOnly name)) shared);
        }
      );
      # What the test and example files share, in the subdirectories of
      # tests/ and examples/ (an example may include tests/support too).
      shared = lib.fileset.unions (
        map
          (
            kind:
            let
              kindDir = dir + "/${kind}";
            in
            lib.fileset.difference (lib.fileset.maybeMissing kindDir) (
              lib.fileset.unions (map (f: kindDir + "/${f}.rs") (targetsIn name kind))
            )
          )
          [
            "tests"
            "examples"
          ]
      );
      target =
        kind: file:
        let
          kindDir = dir + "/${kind}";
        in
        build p (
          removeAttrs crate [ "build" ]
          // {
            crateName = "${name}-${lib.removeSuffix "s" kind}-${file}";
            memberName = name;
            buildTests = kind == "tests";
            crateBin = lib.optional (kind == "examples") {
              name = file;
              path = "examples/${file}.rs";
            };
            libPath = "no-library";
            dependencies = (crate.dependencies or [ ]) ++ (crate.devDependencies or [ ]) ++ [ library ];
            src = targetSrc name (
              lib.fileset.unions [
                (dir + "/Cargo.toml")
                (kindDir + "/${file}.rs")
                shared
              ]
            );
            sourceRoot = "source/crates/${name}";
            preBuild = ''
              if [ -d ${library}/bin ]; then
                mkdir -p target/cargo-bin-exe
                cp ${library}/bin/* target/cargo-bin-exe/
              fi
            '';
            CARGO_CRATE_NAME = file;
            CARGO = "${toolchain}/bin/cargo";
            CARGO_TARGET_TMPDIR = targetTmpDir;
            lints = workspace.lints;
          }
          // memberFlags name crate
        );
    in
    pkgs.symlinkJoin {
      name = "rust_${name}-tests";
      paths = [
        unit
      ]
      ++ map (target "tests") (targetsIn name "tests")
      ++ map (target "examples") (targetsIn name "examples");
    };

  # A member's tree with its tests split (splitTests), each crate built by
  # `build`: the tests' binaries, or with clippy, the lints of its tests
  # and examples.
  testTree =
    name: build:
    let
      packageId = cargoNix.workspaceMembers.${name}.packageId;
    in
    (cargoNix.internal.builtRustCratesWithFeatures {
      inherit packageId;
      crateConfigs = withoutSelf name;
      features = [ "default" ] ++ selfTestFeatures name ++ testFeatures.${name} or [ ];
      runTests = true;
      buildRustCrateForPkgsFunc =
        p: crate: if crate.crateName == name then splitTests name build p crate else build p crate;
    }).crates.${packageId};
  testBinaries = name: testTree name buildRustCrateForPkgs;

  # A member's library and binaries, linted.
  clippyTree =
    name:
    let
      packageId = cargoNix.workspaceMembers.${name}.packageId;
    in
    (cargoNix.internal.builtRustCratesWithFeatures {
      inherit packageId;
      features = [ "default" ];
      runTests = false;
      buildRustCrateForPkgsFunc = buildWithClippy;
    }).crates.${packageId};

  # Cargo's metadata of the workspace, which nextest reads to place the
  # binaries it runs: from its manifests alone, with dummy targets, so that
  # editing code leaves it, and every test run built on it, as it was.
  manifests = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (root + "/Cargo.toml")
      (root + "/Cargo.lock")
      (lib.fileset.fileFilter (file: file.name == "Cargo.toml") (root + "/crates"))
    ];
  };
  cargoMetadata = pkgs.runCommand "cargo-metadata" { nativeBuildInputs = [ toolchain ]; } ''
    cp -r --no-preserve=mode ${craneLib.mkDummySrc { src = manifests; }} $NIX_BUILD_TOP/source
    cd $NIX_BUILD_TOP/source
    CARGO_HOME=$TMPDIR cargo metadata --offline --no-deps --format-version 1 > $out
  '';

  # Runs a member's test binaries with nextest, on .config/nextest.toml's
  # ci profile (every test in a process of its own), where they were
  # compiled, so that what they read through CARGO_MANIFEST_DIR is there:
  # the member's source, with the property tests' fixed seed
  # (.cargo/config.toml). systemd-socket-activate for dassie-cli's process
  # tests, which skip without it.
  # The members with no tests, whose run passes with none; any other
  # member's fails, so that tests that are no longer found fail the check.
  withoutTests = [ "dassie-wasm" ];
  runTests =
    name:
    let
      binaries = testBinaries name;
      underscored = lib.replaceStrings [ "-" ] [ "_" ] name;
      triple = pkgs.stdenv.hostPlatform.rust.rustcTarget;
    in
    pkgs.runCommand "crate-test-${name}"
      {
        nativeBuildInputs = [
          pkgs.cargo-nextest
          pkgs.jq
          pkgs.systemd
          toolchain
        ];
        PROPTEST_RNG_SEED = "0";
      }
      ''
        cd "$NIX_BUILD_TOP"
        cp -r --no-preserve=mode ${memberSrc name true} source
        cp ${root + "/Cargo.toml"} source/Cargo.toml
        mkdir -p ${targetTmpDir}
        package=$(jq -r '.packages[] | select(.name == "${name}") | .id' ${cargoMetadata})
        # The binaries as nextest lists those it built: the library's unit
        # tests, and each file in tests/. Examples are built, not run.
        for binary in ${binaries}/tests/*; do
          [ -e "$binary" ] || continue
          base=$(basename "$binary")
          case "$base" in
            ${underscored}-*) jq -n --arg path "$binary" --arg pkg "$package" \
                '{"${name}": {"binary-id": "${name}", "binary-name": "${underscored}", "package-id": $pkg, "kind": "lib", "binary-path": $path, "build-platform": "target"}}' ;;
            *) jq -n --arg path "$binary" --arg pkg "$package" --arg base "$base" \
                '{("${name}::" + $base): {"binary-id": ("${name}::" + $base), "binary-name": $base, "package-id": $pkg, "kind": "test", "binary-path": $path, "build-platform": "target"}}' ;;
          esac
        done | jq -s --arg target "$NIX_BUILD_TOP/target" --arg triple "${triple}" '{
          "rust-build-meta": {
            "target-directory": $target, "build-directory": $target,
            "base-output-directories": ["ci"], "non-test-binaries": {},
            "build-script-out-dirs": {}, "build-script-info": {}, "linked-paths": [],
            "platforms": {"host": {"platform": {"triple": $triple, "target-features": "unknown"}, "libdir": {"status": "unavailable", "reason": "not-in-archive"}}, "targets": []},
            "target-platforms": [{"triple": $triple, "target-features": "unknown"}],
            "target-platform": null
          },
          "rust-binaries": (add // {})
        }' > binaries.json
        export HOME=$TMPDIR RUST_BACKTRACE=1
        cargo-nextest nextest run --profile ci --config-file ${root + "/.config/nextest.toml"} \
          --no-tests=${if lib.elem name withoutTests then "pass" else "fail"} \
          --color never --show-progress none \
          --binaries-metadata binaries.json --cargo-metadata ${cargoMetadata} \
          --workspace-remap "$NIX_BUILD_TOP/source"
        touch $out
      '';

  # Cargo.nix is what `nix run .#cargo-nix` makes of the workspace now:
  # crate2nix, offline, with the dependencies' sources vendored from
  # Cargo.lock, on the crates whole, whose targets it reads.
  workspaceSrc = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (root + "/Cargo.toml")
      (root + "/Cargo.lock")
      (root + "/crates")
    ];
  };
  cargoNixCurrent =
    pkgs.runCommand "cargo-nix-current"
      {
        nativeBuildInputs = [
          pkgs.crate2nix
          toolchain
        ];
        CARGO_NET_OFFLINE = "true";
      }
      ''
        cp -r --no-preserve=mode ${workspaceSrc} source
        cd source
        export HOME=$TMPDIR CARGO_HOME=$TMPDIR/cargo-home
        mkdir -p $CARGO_HOME nix/rust
        cp ${
          craneLib.vendorCargoDeps { cargoLock = root + "/Cargo.lock"; }
        }/config.toml $CARGO_HOME/config.toml
        crate2nix generate -f Cargo.toml -o nix/rust/Cargo.nix
        if ! diff -u ${./Cargo.nix} nix/rust/Cargo.nix; then
          echo "nix/rust/Cargo.nix is stale: run 'nix run .#cargo-nix' and commit nix/rust/Cargo.nix" >&2
          exit 1
        fi
        touch $out
      '';
  # buildRustCrate runs no doc tests, so no doc example is one: a fence in
  # a doc comment names a language other than Rust (`text`, `json`) or is
  # `ignore`d, and what an example shows is tested in the crate's tests.
  rustSources = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.fileFilter (file: file.hasExt "rs") (root + "/crates");
  };
  docExamples = pkgs.runCommand "doc-examples" { } ''
    cd ${rustSources}
    find crates -name '*.rs' | sort | xargs awk '
      FNR == 1 { open = 0 }
      /^[[:space:]]*\/\/[\/!][[:space:]]*```/ {
        if (open) { open = 0; next }
        open = 1
        tag = $0
        sub(/.*```[[:space:]]*/, "", tag)
        sub(/[[:space:]]+$/, "", tag)
        n = split(tag, words, ",")
        test = tag == ""
        ignored = 0
        for (i = 1; i <= n; i++) {
          if (words[i] ~ /^(rust|no_run|should_panic|compile_fail|edition[0-9]+)$/) test = 1
          if (words[i] == "ignore") ignored = 1
        }
        if (test && !ignored) {
          printf "%s:%d: a doc test, which CI never runs: test it in the crate\x27s tests, and mark the example `ignore`\n", FILENAME, FNR
          found = 1
        }
      }
      END { exit found }
    '
    touch $out
  '';
in
{
  inherit testBinaries runTests;

  # After a change to Cargo.lock, a manifest or a crate's targets.
  generate = pkgs.writeShellApplication {
    name = "cargo-nix";
    runtimeInputs = [
      pkgs.crate2nix
      toolchain
    ];
    text = "crate2nix generate -f Cargo.toml -o nix/rust/Cargo.nix";
  };

  checks =
    lib.concatMapAttrs (name: _: {
      "crate-${name}-test" = runTests name;
      "crate-${name}-clippy" = clippyTree name;
      "crate-${name}-clippy-tests" = testTree name buildWithClippy;
    }) (lib.genAttrs members (_: null))
    // {
      cargo-nix-current = cargoNixCurrent;
      doc-examples = docExamples;
    };
}
