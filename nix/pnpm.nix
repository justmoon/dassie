# The pnpm workspace's npm and JSR packages, for builds in the sandbox:
# one fetchurl per package pnpm-lock.yaml pins, checked against the
# integrity the lockfile records for it, and served to `pnpm install` by a
# registry mirror on loopback (registry-mirror.mjs). A change to the lockfile
# needs no hash set by hand: each package's hash is the lockfile's own.
#
# The lockfile is YAML, which Nix can't read. Its layout is regular enough
# to read here, line by line, so that evaluating the flake builds nothing
# (no import from derivation). What is read: the importers' dependencies,
# each package's resolution and platform, and each snapshot's dependencies.
{ lib, pkgs }:
let
  root = ../.;
  lockfile = root + "/pnpm-lock.yaml";

  # 'quoted' -> quoted, with YAML's '' for a quote.
  unquote =
    s:
    let
      m = builtins.match "'(.*)'" s;
    in
    if m == null then s else builtins.replaceStrings [ "''" ] [ "'" ] (lib.head m);

  # Entries are separated by blank lines; a section starts with an
  # unindented `name:`, an entry with a key indented by two spaces.
  sections = lib.foldl' (
    acc: block:
    let
      lines = lib.splitString "\n" block;
      section = builtins.match "([a-zA-Z]+):.*" (lib.head lines);
      entry = builtins.match "  ([^ ].*):( \\{})?" (lib.head lines);
    in
    if section != null then
      acc // { current = lib.head section; }
    else if entry != null then
      acc
      // {
        ${acc.current} = (acc.${acc.current} or [ ]) ++ [
          {
            name = unquote (lib.head entry);
            value = lib.tail lines;
          }
        ];
      }
    else
      acc
  ) { current = null; } (lib.splitString "\n\n" (builtins.readFile lockfile));

  # A dependency's snapshot key: `name@version(peers)`, or the alias's
  # target (`'@std/assert': '@jsr/std__assert@1.0.19'`). Workspace links
  # have none.
  depPath =
    name: version:
    if lib.hasPrefix "link:" version then
      null
    else if builtins.match "@?[^@]+@.*" (lib.head (lib.splitString "(" version)) != null then
      version
    else
      "${name}@${version}";
  # The package a snapshot is of: its key without the peers.
  packageOf = path: lib.head (lib.splitString "(" path);

  # The dependencies an importer or a snapshot lists, as { name, version }:
  # `name: version` in a snapshot, `name:` then `version:` in an importer.
  # Peer and transitive peer lists are not edges.
  depKinds = [
    "dependencies"
    "devDependencies"
    "optionalDependencies"
  ];
  depsOf =
    lines:
    (lib.foldl'
      (
        acc: line:
        let
          kind = builtins.match "    ([a-zA-Z]+):.*" line;
          pair = builtins.match "      ([^ ].*): (.+)" line;
          name = builtins.match "      ([^ ].*):" line;
          version = builtins.match "        version: (.+)" line;
          add =
            n: v:
            acc
            // lib.optionalAttrs (lib.elem acc.kind depKinds) {
              deps = acc.deps ++ [
                {
                  name = unquote n;
                  version = unquote v;
                }
              ];
            };
        in
        if kind != null then
          acc // { kind = lib.head kind; }
        else if pair != null then
          add (lib.elemAt pair 0) (lib.elemAt pair 1)
        else if name != null then
          acc // { name = lib.head name; }
        else if version != null then
          add acc.name (lib.head version)
        else
          acc
      )
      {
        kind = null;
        name = null;
        deps = [ ];
      }
      lines
    ).deps;

  importers = lib.listToAttrs sections.importers;
  snapshots = lib.listToAttrs sections.snapshots;

  # Each package's tarball and platform. A package from the npm registry
  # has no URL in the lockfile; pnpm makes it from the name and version, as
  # here. JSR's are recorded.
  packages = lib.mapAttrs (
    key: lines:
    let
      # `    <f>: <value>`, and `[a, b]` as a list.
      value =
        f:
        let
          m = lib.findFirst (m: m != null) null (map (builtins.match "    ${f}: (.*)") lines);
        in
        if m == null then null else lib.head m;
      list =
        f:
        if value f == null then
          null
        else
          map unquote (lib.splitString ", " (lib.head (builtins.match "\\[(.*)]" (value f))));
      resolution = value "resolution";
      integrity = builtins.match ".*integrity: ([^,}]+).*" resolution;
      tarball = builtins.match ".*tarball: ([^,}]+).*" resolution;
      name = lib.head (builtins.match "(@?[^@]+)@.*" key);
      version = lib.removePrefix "${name}@" key;
    in
    assert lib.assertMsg (integrity != null) "pnpm-lock.yaml: ${key} has no integrity";
    {
      url =
        if tarball != null then
          lib.head tarball
        else
          "https://registry.npmjs.org/${name}/-/${baseNameOf name}-${version}.tgz";
      hash = lib.head integrity;
      cpu = list "cpu";
      os = list "os";
      libc = list "libc";
    }
  ) (lib.listToAttrs sections.packages);

  # What the sandbox's pnpm installs: packages for x86_64 Linux with glibc,
  # as Node in nixpkgs is. Packages for other platforms are optional ones
  # that pnpm skips.
  allows =
    wanted: list:
    list == null
    || (
      if lib.all (lib.hasPrefix "!") list then !(lib.elem "!${wanted}" list) else lib.elem wanted list
    );
  forPlatform = p: allows "x64" p.cpu && allows "linux" p.os && allows "glibc" p.libc;

  # `dir/link` with `..` resolved: an importer's link to another.
  resolve =
    dir: link:
    lib.concatStringsSep "/" (
      lib.foldl' (
        acc: segment:
        if segment == ".." then
          lib.init acc
        else if segment == "." || segment == "" then
          acc
        else
          acc ++ [ segment ]
      ) [ ] (lib.splitString "/" "${dir}/${link}")
    );

  # The packages that `pnpm install --filter <dir's package>...` installs:
  # those of the importer, of the importers it links and of the root (which
  # a filtered install includes too), and theirs, for this platform.
  packagesFor =
    dir:
    let
      members = map (m: m.key) (
        builtins.genericClosure {
          startSet = [
            { key = dir; }
            { key = "."; }
          ];
          operator =
            m:
            map (d: { key = resolve m.key (lib.removePrefix "link:" d.version); }) (
              lib.filter (d: lib.hasPrefix "link:" d.version) (depsOf importers.${m.key})
            );
        }
      );
      edges =
        deps:
        lib.filter (path: path != null && forPlatform packages.${packageOf path}) (
          map (d: depPath d.name d.version) deps
        );
      snapshotClosure = builtins.genericClosure {
        startSet = map (key: { inherit key; }) (lib.concatMap (m: edges (depsOf importers.${m})) members);
        operator = s: map (key: { inherit key; }) (edges (depsOf snapshots.${s.key}));
      };
    in
    lib.unique (map (s: packageOf s.key) snapshotClosure);

  # The mirror's tree: each tarball at its URL's host and path, so that
  # http://<mirror>/registry.npmjs.org/ stands for the npm registry and
  # http://<mirror>/npm.jsr.io/ for JSR's.
  treeFor =
    dir:
    pkgs.linkFarm "pnpm-packages" (
      map (
        key:
        let
          p = packages.${key};
        in
        {
          name = lib.removePrefix "https://" p.url;
          path = pkgs.fetchurl { inherit (p) url hash; };
        }
      ) (packagesFor dir)
    );
in
{
  # What `pnpm install --frozen-lockfile` reads: the lockfile and every
  # member's manifest, even those it doesn't install.
  manifests = lib.fileset.unions (
    [
      lockfile
      (root + "/pnpm-workspace.yaml")
      (root + "/.npmrc")
    ]
    ++ map (dir: root + "/${dir}/package.json") (lib.attrNames importers)
  );

  # Installs the dependencies of the workspace member in `dir` (and of the
  # members it depends on) into node_modules, offline: run from the
  # workspace's root, with Node and pnpm on PATH.
  #
  # The lockfile's recorded URLs are pointed at the mirror. pnpm verifies a
  # lockfile against the release-age rule and registry metadata unless it
  # trusts it, and there is no metadata here; every tarball is checked
  # against the lockfile's integrity, by Nix and again by pnpm.
  install =
    dir:
    let
      name = (lib.importJSON (root + "/${dir}/package.json")).name;
    in
    ''
      node ${./registry-mirror.mjs} ${treeFor dir} "$TMPDIR/mirror-port" &
      mirror=$!
      while [ ! -s "$TMPDIR/mirror-port" ]; do
        kill -0 $mirror
        sleep 0.1
      done
      mirror_url=http://127.0.0.1:$(cat "$TMPDIR/mirror-port")
      sed -i "s|tarball: https://|tarball: $mirror_url/|" pnpm-lock.yaml
      pnpm install --frozen-lockfile --filter '${name}...' \
        --config.update-notifier=false \
        --config.registry=$mirror_url/registry.npmjs.org/ \
        --config.@jsr:registry=$mirror_url/npm.jsr.io/ \
        --config.trust-lockfile=true \
        --store-dir "$TMPDIR/pnpm-store"
      kill $mirror
    '';
}
