# The Deno workspace's npm and JSR dependencies, fetched one package (npm) or
# one file (JSR) at a time, each checked against a hash deno.lock vouches
# for: npm tarballs by their integrity in deno.lock, JSR files by the
# `_meta.json` whose hash deno.lock records (listed ahead in jsr.json).
#
# They're served to `deno install --frozen` by a registry mirror on loopback
# inside the build sandbox (mirror.py), so Deno lays out its cache and
# node_modules itself, as on a developer's machine. After Vorstand's
# nix/deno (D14).
{ pkgs, deno }:
let
  inherit (pkgs) lib;
  root = ../..;
  lock = lib.importJSON (root + "/deno.lock");
  jsrFiles = lib.importJSON ./jsr.json;

  # "name@version_peer+suffix" -> { name, version }; names may start with @.
  splitKey =
    key:
    let
      m = builtins.match "(@?[^@]+)@([^_]+)(_.*)?" key;
    in
    {
      name = builtins.elemAt m 0;
      version = builtins.elemAt m 1;
    };

  # Every locked npm package, those for other platforms too: Deno skips them
  # when installing, but resolving still looks them up.
  npmTarballs = lib.unique (
    lib.mapAttrsToList (
      key: p:
      let
        k = splitKey key;
      in
      {
        inherit (k) name version;
        file = pkgs.fetchurl {
          url = "https://registry.npmjs.org/${k.name}/-/${baseNameOf k.name}-${k.version}.tgz";
          hash = p.integrity;
        };
      }
    ) lock.npm
  );

  jsrPackages =
    assert lib.assertMsg (lib.attrNames lock.jsr == lib.attrNames jsrFiles)
      "nix/deno/jsr.json doesn't list deno.lock's JSR versions: run 'nix run .#deno-jsr' and commit nix/deno/jsr.json";
    lib.mapAttrsToList (
      key: p:
      let
        k = splitKey key;
        base = "https://jsr.io/${k.name}/${k.version}";
      in
      {
        inherit (k) name version;
        meta = pkgs.fetchurl {
          url = "${base}_meta.json";
          sha256 = p.integrity;
        };
        listed = builtins.toFile "jsr-files" (lib.concatLines (lib.attrNames jsrFiles.${key}));
        files = lib.mapAttrsToList (path: checksum: {
          inherit path;
          file = pkgs.fetchurl {
            url = "${base}${path}";
            sha256 = lib.removePrefix "sha256-" checksum;
          };
        }) jsrFiles.${key};
      }
    ) lock.jsr;

  # The mirror's tree: npm/<name>/-/<file>.tgz, jsr/<name>/<version>_meta.json
  # and jsr/<name>/<version>/<path>. Packuments and JSR package metadata
  # change over time upstream, so mirror.py makes its own from what's here.
  tree = pkgs.runCommand "deno-deps-tree" { nativeBuildInputs = [ pkgs.jq ]; } (
    ''
      mkdir -p $out
    ''
    + lib.concatMapStrings (t: ''
      install -Dm644 ${t.file} "$out/npm/${t.name}/-/${baseNameOf t.name}-${t.version}.tgz"
    '') npmTarballs
    + lib.concatMapStrings (
      j:
      ''
        jq -r '.manifest | keys[]' ${j.meta} | cmp - ${j.listed} \
          || { echo "jsr.json lists other files than ${j.name}@${j.version}'s _meta.json" >&2; exit 1; }
        install -Dm644 ${j.meta} "$out/jsr/${j.name}/${j.version}_meta.json"
      ''
      + lib.concatMapStrings (f: ''
        install -Dm644 ${f.file} "$out/jsr/${j.name}/${j.version}${f.path}"
      '') j.files
    ) jsrPackages
  );

  # Runs a command with the mirror up and Deno pointed at it.
  withMirror = command: ''
    ${pkgs.python3}/bin/python3 ${./mirror.py} ${tree} 127.0.0.1:4873 &
    mirror=$!
    for _ in $(seq 50); do
      ${pkgs.curl}/bin/curl -sf http://127.0.0.1:4873/healthz >/dev/null && break
      sleep 0.1
    done
    export NPM_CONFIG_REGISTRY=http://127.0.0.1:4873/npm/ JSR_URL=http://127.0.0.1:4873/
    ${command}
    kill $mirror
  '';

  # The workspace's manifests: what `deno install` reads.
  manifests = [
    (root + "/deno.json")
    (root + "/deno.lock")
  ]
  ++ map (m: root + "/${m}/deno.json") (lib.importJSON (root + "/deno.json")).workspace;
in
{
  inherit tree withMirror manifests;

  # Every member's dependencies, installed: node_modules and Deno's cache.
  # Keyed only on the manifests and the lockfile.
  installed = pkgs.stdenvNoCC.mkDerivation {
    name = "deno-install";
    src = lib.fileset.toSource {
      inherit root;
      fileset = lib.fileset.unions manifests;
    };
    nativeBuildInputs = [ deno ];
    buildPhase = withMirror ''
      export DENO_DIR=$out/deno-dir HOME=$TMPDIR DENO_NO_UPDATE_CHECK=1
      deno install --frozen
    '';
    installPhase = "cp -a node_modules $out/node_modules";
    dontFixup = true;
  };
}
