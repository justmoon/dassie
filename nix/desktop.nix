# The desktop app (crates/dassie-desktop, spec/areas/desktop.md): the
# owner's node in a window, for Linux, as a Nix package
# (spec/decisions/desktop-app.md). Unlike `dassie` it links glibc and the
# system's GTK and WebKitGTK, which the wrapper finds with their GIO
# modules (glib-networking: WebKit's TLS) and schemas. The tray icon's
# AppIndicator library is loaded at run time, so the wrapper puts it on the
# library path.
#
# The package builds in release, as owners install it. With `check`, the
# flake's check builds it in the ci profile (Cargo.toml), and first runs
# clippy and the app's tests on the same build, so that the workspace's
# crates compile once for all three: the code is the same, every
# `debug_assertions` in it a run-time switch, and how well the app is
# optimized is not what the check checks.
{
  lib,
  pkgs,
  craneLib,
  src,
  wallet,
  check ? false,
}:
let
  desktopLibs = import ./desktop-libs.nix { inherit pkgs; };
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;
  common = {
    inherit version;
    src =
      if check then
        lib.fileset.toSource {
          root = ../.;
          fileset = lib.fileset.unions [
            (lib.fileset.fromSource src)
            ../.config/nextest.toml
          ];
        }
      else
        src;
    pname = "dassie-desktop";
    strictDeps = true;
    cargoExtraArgs = "--locked -p dassie-desktop";
    CARGO_PROFILE = if check then "ci" else "release";
    doCheck = false;
    nativeBuildInputs =
      desktopLibs.nativeBuildInputs ++ [ pkgs.wrapGAppsHook3 ] ++ lib.optional check pkgs.cargo-nextest;
    buildInputs = desktopLibs.buildInputs ++ [
      pkgs.glib-networking
      pkgs.gsettings-desktop-schemas
    ];
  };
in
craneLib.buildPackage (
  common
  // {
    cargoArtifacts = craneLib.buildDepsOnly common;
    DASSIE_WALLET_DIST = wallet;
    DASSIE_BUILD_VERSION = version;
    preBuild = lib.optionalString check ''
      cargoWithProfile clippy --locked -p dassie-desktop --all-targets -- -D warnings
      cargo nextest run --cargo-profile ci --profile ci --locked -p dassie-desktop \
        --color never --show-progress none
    '';
    preFixup = ''
      gappsWrapperArgs+=(--prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath [ pkgs.libayatana-appindicator ]})
    '';
    postInstall = ''
      install -Dm644 ${../crates/dassie-desktop/resources/land.dassie.desktop.desktop} \
        $out/share/applications/land.dassie.desktop.desktop
      for size in 32 128 512; do
        install -Dm644 ${../crates/dassie-desktop/icons}/$size.png \
          $out/share/icons/hicolor/''${size}x''${size}/apps/land.dassie.desktop.png
      done
    '';
    meta = {
      description = "Dassie: your node, with its wallet, on your desktop";
      mainProgram = "dassie-desktop";
      platforms = [ "x86_64-linux" ];
    };
  }
)
