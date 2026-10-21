{
  # Dassie for NixOS, from its mirror (spec/areas/release.md#nixos,
  # spec/decisions/nix-flake-on-the-mirror.md). Each release publishes
  # this flake as <version>/flake.tar.gz, beside its bundles, with
  # release.json naming the version and each bundle's sum: the package is
  # the release's own signed binary, and nothing is compiled.
  #
  #   inputs.dassie.url = "https://get.dassie.land/<version>/flake.tar.gz";
  #   inputs.dassie.inputs.nixpkgs.follows = "nixpkgs";
  #
  #   modules = [ dassie.nixosModules.default { services.dassie.enable = true; } ];
  description = "Dassie: a multi-currency, peer-to-peer, Interledger-enabled payment network";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      release = builtins.fromJSON (builtins.readFile ./release.json);
      systems = builtins.attrNames release.bundles;
      dassieFor = pkgs: pkgs.callPackage ./package.nix { inherit release; };
    in
    {
      packages = nixpkgs.lib.genAttrs systems (
        system:
        let
          dassie = dassieFor nixpkgs.legacyPackages.${system};
        in
        {
          inherit dassie;
          default = dassie;
        }
      );

      overlays.default = final: _prev: { dassie = dassieFor final; };

      # The module, with this release as its package.
      nixosModules.default =
        { lib, pkgs, ... }:
        {
          imports = [ ./module.nix ];
          services.dassie.package = lib.mkDefault (dassieFor pkgs);
        };
      nixosModules.dassie = self.nixosModules.default;
    };
}
