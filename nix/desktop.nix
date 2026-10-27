# The desktop app (crates/dassie-desktop, spec/areas/desktop.md): the
# owner's node in a window, for Linux, as a Nix package
# (spec/decisions/desktop-app.md). Unlike `dassie` it links glibc and the
# system's GTK and WebKitGTK, which the wrapper finds with their GIO
# modules (glib-networking: WebKit's TLS) and schemas. The tray icon's
# AppIndicator library is loaded at run time, so the wrapper puts it on the
# library path.
#
# The package builds in release, as owners install it. Given `binary`, the
# app nix/rust built for the checks, it is wrapped as the package is
# instead: the flake's check, that the app links with the wallet built in
# and is wrapped, without building it again.
{
  lib,
  pkgs,
  craneLib,
  src,
  wallet,
  binary ? null,
}:
let
  desktopLibs = import ./desktop-libs.nix { inherit pkgs; };
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;
  nativeBuildInputs = desktopLibs.nativeBuildInputs ++ [ pkgs.wrapGAppsHook3 ];
  buildInputs = desktopLibs.buildInputs ++ [
    pkgs.glib-networking
    pkgs.gsettings-desktop-schemas
  ];
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
  common = {
    inherit
      src
      version
      nativeBuildInputs
      buildInputs
      ;
    pname = "dassie-desktop";
    strictDeps = true;
    cargoExtraArgs = "--locked -p dassie-desktop";
    CARGO_PROFILE = "release";
    doCheck = false;
  };
in
if binary != null then
  pkgs.stdenv.mkDerivation {
    pname = "dassie-desktop";
    inherit
      version
      nativeBuildInputs
      buildInputs
      preFixup
      postInstall
      meta
      ;
    dontUnpack = true;
    dontBuild = true;
    installPhase = ''
      runHook preInstall
      install -Dm755 ${binary}/bin/dassie-desktop $out/bin/dassie-desktop
      runHook postInstall
    '';
  }
else
  craneLib.buildPackage (
    common
    // {
      cargoArtifacts = craneLib.buildDepsOnly common;
      DASSIE_WALLET_DIST = wallet;
      DASSIE_BUILD_VERSION = version;
      inherit preFixup postInstall meta;
    }
  )
