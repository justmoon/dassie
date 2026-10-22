# The Dassie node on NixOS (spec/areas/release.md#nixos): the release's own
# systemd units, from the package, enabled as install.sh enables them. The
# units hold the node's sandbox, so it is the same on NixOS as anywhere;
# what is set here goes into drop-ins. There is no updater: the flake's
# version is the node's.
{
  config,
  lib,
  ...
}:
let
  cfg = config.services.dassie;
  sockets = [
    "dassie-http"
    "dassie-https"
    "dassie-ipc"
    "dassie-quic"
    "dassie-feed"
  ];
in
{
  options.services.dassie = {
    enable = lib.mkEnableOption "the Dassie node, on ports 80 and 443";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The Dassie package: the flake's release, by default.";
    };

    logLevel = lib.mkOption {
      type = lib.types.enum [
        "debug"
        "info"
        "warn"
        "error"
        "none"
      ];
      default = "info";
      description = "How much the node logs (`DASSIE_LOG_LEVEL`).";
    };

    environment = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      example = {
        DASSIE_RELAY = "";
      };
      description = "Further environment variables for the node, such as `DASSIE_RELAY`.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open TCP ports 80 and 443, and UDP ports 443 (QUIC) and 7768 (the rate feed), in the firewall.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.packages = [ cfg.package ];
    systemd.services.dassie = {
      wantedBy = [ "multi-user.target" ];
      environment = {
        DASSIE_LOG_LEVEL = cfg.logLevel;
      }
      // cfg.environment;
    };
    systemd.sockets = lib.genAttrs sockets (_: {
      wantedBy = [ "sockets.target" ];
    });

    # Whoever is in it may use the IPC socket, and so administers the node
    # (`dassie init`, `dassie status`).
    users.groups.dassie-users = { };
    environment.systemPackages = [ cfg.package ];

    networking.firewall = lib.mkIf cfg.openFirewall {
      allowedTCPPorts = [
        80
        443
      ];
      allowedUDPPorts = [
        443
        7768
      ];
    };
  };
}
