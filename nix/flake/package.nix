# A release's `dassie` for one platform: its bundle from the mirror, by the
# sum release.json gives (the sum SHA256SUMS lists, which the release key
# signed), unpacked as it is. The binary is static and runs on NixOS
# unchanged. The bundle's systemd units go where NixOS finds a package's
# units, running this binary rather than /opt/dassie/current's; the
# updater's units stay out, since Nix decides the version.
{
  lib,
  stdenvNoCC,
  fetchurl,
  release,
  # The bundle itself, in place of the mirror's: for tests.
  src ? null,
}:
let
  inherit (stdenvNoCC.hostPlatform) system;
  bundle =
    release.bundles.${system}
      or (throw "dassie: release ${release.version} has no bundle for ${system}");
in
stdenvNoCC.mkDerivation {
  pname = "dassie";
  inherit (release) version;

  src =
    if src != null then
      src
    else
      fetchurl {
        url = "${release.mirror}/${release.version}/${bundle.file}";
        inherit (bundle) sha256;
      };
  sourceRoot = "dassie";

  dontConfigure = true;
  dontBuild = true;
  # The binary is installed as the release built it: nothing strips or
  # patches it.
  dontFixup = true;

  installPhase = ''
    runHook preInstall
    install -Dm755 bin/dassie $out/bin/dassie
    mkdir -p $out/lib/systemd/system
    for unit in share/systemd/*; do
      name=$(basename "$unit")
      case "$name" in
        dassie-update.*) continue ;;
      esac
      sed "s|/opt/dassie/current/bin/dassie|$out/bin/dassie|g" "$unit" \
        > "$out/lib/systemd/system/$name"
    done
    grep -q "^ExecStart=$out/bin/dassie daemon " $out/lib/systemd/system/dassie.service
    runHook postInstall
  '';

  meta = {
    description = "Dassie node: a multi-currency, peer-to-peer, Interledger-enabled payment network";
    homepage = "https://dassie.land";
    license = lib.licenses.asl20;
    platforms = builtins.attrNames release.bundles;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    mainProgram = "dassie";
  };
}
