# The Cargo workspace (crates/) as one derivation per crate, so that an
# edit rebuilds the crate edited and the crates built on it, and runs only
# their tests again; a lockfile bump rebuilds the dependencies it moved and
# what is built on them (spec/decisions/crate-by-crate-builds.md). Cargo.nix
# is crate2nix's description of the workspace, committed and held to
# Cargo.lock by a check (`nix run .#cargo-nix` after a change to a manifest
# or the lockfile); nixpkgs' buildRustCrate builds it.
#
# Every crate builds once: with the features the whole workspace asks of
# it, the workspace's own with clippy-driver, which lints them as they
# compile. For each member, as a check, its tests, run with nextest
# (crate-<name>-test), which builds its library, binaries, tests and
# examples, and so lints them all.
{
  pkgs,
  toolchain,
  craneLib,
  # The built wallet, which the binaries embed, where it is built.
  wallet ? null,
}:
let
  inherit (pkgs) lib;
  root = ../..;
  workspace = (lib.importTOML (root + "/Cargo.toml")).workspace;
  # Every member, the desktop app included (keepBuildDir, depEnv).
  members = map (lib.removePrefix "crates/") workspace.members;

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
    # Tauri's build script reads the version the crate takes from the
    # workspace.
    dassie-desktop = [ (root + "/Cargo.toml") ];
    # The node's table of media types, which its build script shares.
    dassie-wallet-assets = [ (root + "/crates/dassie-node/src/http/mime.rs") ];
  };
  testExtraFiles = {
    dassie-feed = [ (root + "/testdata/feed") ];
    dassie-hub = [
      (root + "/crates/dassie-identity/tests/data/paths.json")
      (root + "/testdata/hub")
    ];
    dassie-peer = [ (root + "/crates/dassie-oer/tests/data") ];
    dassie-xrpl = [
      (root + "/crates/dassie-identity/tests/data/chain.json")
      (root + "/resources/proof-server/xrpld-mainnet.cfg")
      (root + "/resources/proof-server/xrpld-testnet.cfg")
    ];
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
  ciFlags = debugAssertions: [
    "-C"
    "debuginfo=line-tables-only"
    "-C"
    "overflow-checks=on"
    "-C"
    "debug-assertions=${if debugAssertions then "on" else "off"}"
  ];
  # Cargo compiles from the workspace's root, so a panic, `file!()` and a
  # test's failure name `crates/<name>/src/...`, as the simulation
  # campaign's failure signatures do; buildRustCrate compiles from the
  # crate's directory, and the paths are remapped to Cargo's.
  # dassie-identity is the one member optimized fully: the node's SLH-DSA
  # key generation is `slh-dsa`'s generic code, which is compiled where it
  # is instantiated, in dassie-identity, and takes most of a second at
  # opt-level 1 (spec/decisions/post-quantum-node-id.md), as Cargo's dev
  # profile says too.
  optimizedMembers = [ "dassie-identity" ];
  memberFlags = name: crate: {
    codegenUnits = 16;
    extraRustcOpts =
      (crate.extraRustcOpts or [ ])
      ++ [
        "-C"
        "opt-level=${if lib.elem name optimizedMembers then "3" else "1"}"
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
  # with too, and the property tests' fixed seed (runTests).
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
        # A target of a split test build (tests) brings its own.
        src = crate.targetSrc or (memberSrc name (crate.buildTests or false));
        sourceRoot = "source/crates/${name}";
        # Cargo sets these and buildRustCrate doesn't.
        CARGO_CRATE_NAME = lib.replaceStrings [ "-" ] [ "_" ] name;
        CARGO = "${toolchain}/bin/cargo";
        lints = workspace.lints;
      }
      // memberFlags name crate
    );

  # What a build script hands the crates built on it (`cargo:KEY=VALUE`
  # for a crate with `links`), which Cargo passes to their build scripts as
  # DEP_<LINKS>_<KEY>, buildRustCrate writes as `export` lines to a file
  # they source. Two things Cargo allows that this can't carry, which Tauri
  # relies on, are mended after a crate is built (keepBuildDir) and before
  # one is configured (depEnv):
  # - A path into the crate's build directory, as its OUT_DIR or its
  #   source: Cargo keeps them, a derivation's build directory is gone once
  #   it is built. The directory is kept in the output, without what is
  #   compiled, and the paths name it there.
  # - A name bash won't export, as Tauri's core plugins' `core:path`: it is
  #   written to a file instead, and `env`, through which buildRustCrate
  #   runs a build script, hands it on.
  keepBuildDir = ''
    if [ -s "$lib/env" ] && grep -qF "$NIX_BUILD_TOP/" "$lib/env"; then
      kept=$lib/build
      mkdir -p "$kept"
      tar -C "$NIX_BUILD_TOP" --exclude='*/target/lib' --exclude='*/target/deps' \
        --exclude='*/target/buildDeps' -cf - . | tar -C "$kept" -xf -
      find "$kept" -xtype l -delete
      grep -rlIF "$NIX_BUILD_TOP/" "$kept" "$lib/env" | xargs -r sed -i -E "s#(^|[\"'=: ])$NIX_BUILD_TOP/#\\1$kept/#g"
    fi
    if [ -s "$lib/env" ]; then
      while IFS= read -r line; do
        assignment=''${line#export }
        name=''${assignment%%=*}
        if [[ $line == export\ * && ! $name =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
          value=''${assignment#*=}
          value=''${value#\"}
          value=''${value%\"}
          printf 'echo %q >> "$NIX_BUILD_TOP/.dep-env"\n' "$name=$value"
        else
          printf '%s\n' "$line"
        fi
      done < "$lib/env" > env.mended
      mv env.mended "$lib/env"
    fi
  '';
  depEnv = pkgs.writeShellScriptBin "env" ''
    if [ -s "$NIX_BUILD_TOP/.dep-env" ]; then
      mapfile -t passed < "$NIX_BUILD_TOP/.dep-env"
      exec ${pkgs.coreutils}/bin/env "''${passed[@]}" "$@"
    fi
    exec ${pkgs.coreutils}/bin/env "$@"
  '';

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
          extraRustcOpts = (crate.extraRustcOpts or [ ]) ++ ciFlags (crate.debugAssertions or true);
          preConfigure = ''
            export PATH=${depEnv}/bin:$PATH
          ''
          + (crate.preConfigure or "");
          postInstall = (crate.postInstall or "") + keepBuildDir;
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
  inherit (cargoNix.internal) dependencyDerivations filterEnabledDependencies;
  idOf = name: cargoNix.workspaceMembers.${name}.packageId;
  memberIds = map idOf members;
  isMember = id: lib.elem id memberIds;

  # The features a member turns on for its own tests by depending on itself
  # (`[dev-dependencies] dassie-node = { path = ".", features = [...] }`),
  # which Cargo unifies into the crate its tests build; Cargo.nix's crates
  # leave that dependency out, and its features join the member's own.
  selfTestFeatures =
    name:
    ((lib.importTOML (root + "/crates/${name}/Cargo.toml")).dev-dependencies or { }).${name}.features
      or [ ];
  crateConfigs = lib.mapAttrs (
    id: config:
    config
    // lib.optionalAttrs (isMember id) {
      devDependencies = lib.filter (dep: dep.packageId != id) (config.devDependencies or [ ]);
    }
  ) cargoNix.internal.crates;
  target = cargoNix.internal.makeDefaultTarget pkgs.stdenv.hostPlatform // {
    test = false;
  };

  # Every crate's features, once for the whole workspace, as
  # `cargo test --workspace` unifies them: what each member, its tests
  # included, asks of each crate, together. A crate builds once, with them,
  # for everything built on it; resolving them from each member apart, as
  # crate2nix does, built a crate once for each set of features a member's
  # tree asked of it, and again for each set of its dependencies'
  # (spec/decisions/crate-by-crate-builds.md). `testing` features only add
  # code a test turns on, so the end-to-end binaries are built with them
  # too; dassie-release's tool is one of them.
  extraFeatures = {
    dassie-release = [ "tool" ];
  };
  features = lib.foldl' (
    cache: name:
    cargoNix.internal.mergePackageFeatures {
      inherit crateConfigs target;
      packageId = idOf name;
      features = [ "default" ] ++ selfTestFeatures name ++ extraFeatures.${name} or [ ];
      featuresByPackageId = cache;
      runTests = true;
    }
  ) { } members;
  featuresOf = id: features.${id} or [ ];

  # A crate's dependencies of `kind`, as its features enable them, built.
  depConfigs =
    id: kind:
    filterEnabledDependencies {
      features = featuresOf id;
      inherit target;
      dependencies = crateConfigs.${id}.${kind} or [ ];
    };
  depsOf =
    id: kind:
    dependencyDerivations {
      features = featuresOf id;
      inherit target;
      buildByPackageId = dep: crates.${dep};
      dependencies = crateConfigs.${id}.${kind} or [ ];
    };
  # Dependencies under another name (`foo = { package = "bar" }`).
  renamesOf =
    id: kinds:
    lib.mapAttrs
      (
        _:
        map (dep: {
          inherit (dep) rename;
          inherit (crateConfigs.${dep.packageId}) version;
        })
      )
      (
        lib.groupBy (dep: dep.name) (lib.filter (dep: dep ? rename) (lib.concatMap (depConfigs id) kinds))
      );

  # The crates only the desktop app is built on, Tauri's: they compile with
  # debug assertions off, as the app ships, since Tauri compiles other code
  # with them (its IPC, the runtime authority generate_context! expands,
  # devtools, its embedded assets), and the check is to compile the code
  # owners get. The crates the node shares with the rest keep them; theirs
  # are switches at run time.
  depIds =
    id:
    map (dep: dep.packageId) (
      depConfigs id "dependencies"
      ++ depConfigs id "buildDependencies"
      ++ lib.optionals (isMember id) (depConfigs id "devDependencies")
    );
  closure =
    ids:
    map (crate: crate.key) (
      builtins.genericClosure {
        startSet = map (key: { inherit key; }) ids;
        operator = { key }: map (key: { inherit key; }) (depIds key);
      }
    );
  desktopOnly = lib.subtractLists (closure (lib.remove (idOf "dassie-desktop") memberIds)) (closure [
    (idOf "dassie-desktop")
  ]);

  # What buildRustCrate is given for a crate: its features, its
  # dependencies built, and the wallet for the crate that embeds it.
  argsOf =
    id:
    let
      config = removeAttrs crateConfigs.${id} [
        "resolvedDefaultFeatures"
        "devDependencies"
      ];
    in
    config
    // {
      src =
        config.src or (pkgs.fetchurl {
          name = "${config.crateName}-${config.version}.tar.gz";
          url = "https://static.crates.io/crates/${config.crateName}/${config.crateName}-${config.version}.crate";
          inherit (config) sha256;
        });
      features = featuresOf id;
      dependencies = depsOf id "dependencies";
      buildDependencies = depsOf id "buildDependencies";
      crateRenames = renamesOf id [
        "dependencies"
        "buildDependencies"
      ];
      release = true;
    }
    // lib.optionalAttrs (lib.elem id desktopOnly) { debugAssertions = false; }
    // lib.optionalAttrs (config.crateName == "dassie-wallet-assets" && wallet != null) {
      DASSIE_WALLET_DIST = wallet;
    };
  # Every crate of the lockfile the workspace uses, built once: the
  # workspace's own with clippy-driver, so that building one lints it.
  crates = lib.mapAttrs (id: _: buildWithClippy pkgs (argsOf id)) crateConfigs;

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

  # Features a member's own tests build its library with, beyond the
  # workspace's: the CLI's test key, which trusts testdata/release's key
  # and so is never in a binary anything else runs.
  testFeatures = {
    dassie-cli = [ "test-key" ];
  };

  # A member's tests, each target in a derivation of its own, so that they
  # compile at once and an edit to one test file compiles that one again:
  # buildRustCrate would compile the library, its unit tests and every file
  # in tests/ one after another. They link the library everything else
  # links (with the CLI's test key, one of its own), and its binaries, which
  # integration tests run as CARGO_BIN_EXE_<name>; the unit tests build from
  # its source without tests/ or examples/; each file in tests/, and each
  # example, from the crate without src/, with what the files beside it
  # share (tests/support).
  tests = lib.genAttrs members (
    name:
    let
      id = idOf name;
      dir = root + "/crates/${name}";
      args = argsOf id // {
        features = featuresOf id ++ testFeatures.${name} or [ ];
      };
      library = if testFeatures ? ${name} then buildWithClippy pkgs args else crates.${id};
      testArgs = args // {
        dependencies = args.dependencies ++ depsOf id "devDependencies";
        crateRenames = renamesOf id [
          "dependencies"
          "buildDependencies"
          "devDependencies"
        ];
      };
      unit = buildWithClippy pkgs (
        testArgs
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
      examples = lib.genAttrs (targetsIn name "examples") (target "examples");
      target =
        kind: file:
        let
          kindDir = dir + "/${kind}";
        in
        buildWithClippy pkgs (
          removeAttrs testArgs [ "build" ]
          // {
            crateName = "${name}-${lib.removeSuffix "s" kind}-${file}";
            memberName = name;
            buildTests = kind == "tests";
            crateBin = lib.optional (kind == "examples") {
              name = file;
              path = "examples/${file}.rs";
            };
            libPath = "no-library";
            dependencies = testArgs.dependencies ++ [ library ];
            src = targetSrc name (
              lib.fileset.unions [
                (dir + "/Cargo.toml")
                (kindDir + "/${file}.rs")
                shared
              ]
            );
            sourceRoot = "source/crates/${name}";
            CARGO_CRATE_NAME = file;
            CARGO = "${toolchain}/bin/cargo";
            lints = workspace.lints;
          }
          // memberFlags name testArgs
        );
    in
    pkgs.symlinkJoin {
      name = "rust_${name}-tests";
      paths = [
        unit
      ]
      ++ map (target "tests") (targetsIn name "tests")
      ++ lib.attrValues examples;
      # Each example's own derivation, for what runs one alone (e2e).
      passthru = { inherit examples library; };
    }
  );

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
  withoutTests = [
    "dassie-wallet-assets"
    "dassie-wasm"
  ];
  runTests =
    name:
    let
      binaries = tests.${name};
      inherit (tests.${name}) library;
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
        done > rust-binaries.json
        # The crate's binaries, which nextest names to its integration tests
        # as CARGO_BIN_EXE_<name> when they run: copied where a test may put
        # an install beside them and link one in (dassie-cli's update tests).
        mkdir -p target/ci
        for bin in ${library}/bin/*; do
          [ -e "$bin" ] || continue
          install -m755 "$bin" target/ci/
          jq -n --arg name "$(basename "$bin")" \
            '{"name": $name, "kind": "bin-exe", "path": ("ci/" + $name), "build-platform": "target"}'
        done | jq -s --arg pkg "$package" '{($pkg): .} | with_entries(select(.value != []))' > non-test-binaries.json
        jq -s --arg target "$NIX_BUILD_TOP/target" --arg triple "${triple}" \
          --slurpfile bins non-test-binaries.json '{
          "rust-build-meta": {
            "target-directory": $target, "build-directory": $target,
            "base-output-directories": ["ci"], "non-test-binaries": $bins[0],
            "build-script-out-dirs": {}, "build-script-info": {}, "linked-paths": [],
            "platforms": {"host": {"platform": {"triple": $triple, "target-features": "unknown"}, "libdir": {"status": "unavailable", "reason": "not-in-archive"}}, "targets": []},
            "target-platforms": [{"triple": $triple, "target-features": "unknown"}],
            "target-platform": null
          },
          "rust-binaries": (add // {})
        }' rust-binaries.json > binaries.json
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
  # The binaries the end-to-end tests run, as `node --run build:e2e` lays
  # them out in target/e2e: the node with the wallet built in, dassie-release
  # for the release script's tests, and the fake XRP Ledger and the interop
  # tests' STREAM peer, the examples the crates' tests build. Each was built
  # for the checks already; this only copies them.
  e2eBinaries = pkgs.runCommand "dassie-e2e" { } ''
    mkdir -p $out/examples
    cp ${crates.${idOf "dassie-cli"}}/bin/dassie ${
      crates.${idOf "dassie-release"}
    }/bin/dassie-release $out/
    cp ${tests.dassie-node.examples.fake_ledger}/bin/fake_ledger $out/examples/
    cp ${tests.dassie-stream.examples.interop_peer}/bin/interop_peer $out/examples/
  '';
  # A test finds the files it reads and the binaries it runs from what
  # cargo and nextest name when it runs, never from `env!`'s paths from
  # when it compiled: a build is reused in other checkouts of the
  # repository, as Vorstand Work's sessions start from one already built,
  # and an old checkout's files would be read in their place.
  compileTimePaths = pkgs.runCommand "compile-time-paths" { } ''
    cd ${rustSources}
    if grep -rnE 'env!\("CARGO_(MANIFEST_DIR|TARGET_TMPDIR|BIN_EXE_)' crates; then
      echo "a test compiles in a path; read it when the test runs: std::env::var(\"CARGO_MANIFEST_DIR\") (or CARGO_BIN_EXE_<name>), or tempfile for scratch space" >&2
      exit 1
    fi
    touch $out
  '';
in
{
  inherit runTests e2eBinaries;
  # The desktop app, built for the checks, which the flake wraps.
  desktop = crates.${idOf "dassie-desktop"};

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
    }) (lib.genAttrs members (_: null))
    // {
      cargo-nix-current = cargoNixCurrent;
      doc-examples = docExamples;
      compile-time-paths = compileTimePaths;
    };
}
