# The desktop app (crates/dassie-desktop, spec/areas/desktop.md): the
# owner's node in a window, for Linux, as a Nix package
# (spec/decisions/desktop-app.md). Unlike `dassie` it links glibc and the
# system's GTK and WebKitGTK, which the wrapper finds with their GIO
# modules (glib-networking: WebKit's TLS) and schemas. The tray icon's
# AppIndicator library is loaded at run time, so the wrapper puts it on the
# library path.
#
# `profile` is Cargo's: the package builds in release, as owners install
# it; the flake's check builds the same code in a cheaper profile.
{
  lib,
  pkgs,
  craneLib,
  src,
  wallet,
  profile ? "release",
}:
let
  desktopLibs = import ./desktop-libs.nix { inherit pkgs; };
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;
  common = {
    inherit src version;
    pname = "dassie-desktop";
    strictDeps = true;
    cargoExtraArgs = "--locked -p dassie-desktop";
    CARGO_PROFILE = profile;
    doCheck = false;
    nativeBuildInputs = desktopLibs.nativeBuildInputs ++ [ pkgs.wrapGAppsHook3 ];
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
    DASSIE_VERSION = version;
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
