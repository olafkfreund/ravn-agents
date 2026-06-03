{ config, lib, pkgs, ... }:

let
  inherit (lib) mkEnableOption mkPackageOption mkOption mkIf types optional optionalAttrs;

  cfg = config.services.ravn.controlPlane;

  databaseUrl =
    if cfg.database.url != null then cfg.database.url
    else "postgres://ravn@/ravn?host=/run/postgresql";
in
{
  options.services.ravn.controlPlane = {
    enable = mkEnableOption "the Ravn control plane (ravn-server)";

    package = mkPackageOption pkgs "ravn-server" { };

    bind = mkOption {
      type = types.str;
      default = "127.0.0.1:8080";
      description = ''
        Address the API binds to. Defaults to loopback; put a reverse proxy
        with TLS in front for remote access.
      '';
    };

    logLevel = mkOption {
      type = types.enum [ "error" "warn" "info" "debug" "trace" ];
      default = "info";
      description = "Log verbosity.";
    };

    database = {
      createLocally = mkOption {
        type = types.bool;
        default = true;
        description = "Provision a local PostgreSQL with a `ravn` database.";
      };
      url = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "postgres://ravn:secret@db.example.com/ravn";
        description = ''
          Connection string. Required when `createLocally` is false; otherwise
          defaults to the local socket.
        '';
      };
    };

    nats = {
      createLocally = mkOption {
        type = types.bool;
        default = true;
        description = "Provision a local NATS server (loopback, JetStream).";
      };
      url = mkOption {
        type = types.str;
        default = "nats://127.0.0.1:4222";
        description = "NATS URL the control plane ingests from.";
      };
    };

    oidc = {
      issuer = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "OIDC issuer URL for portal user auth (#26).";
      };
      clientId = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "OIDC client ID (#26).";
      };
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.database.createLocally || cfg.database.url != null;
        message = "services.ravn.controlPlane.database.url must be set when createLocally = false.";
      }
    ];

    users.users.ravn = {
      isSystemUser = true;
      group = "ravn";
    };
    users.groups.ravn = { };

    services.postgresql = mkIf cfg.database.createLocally {
      enable = true;
      ensureDatabases = [ "ravn" ];
      ensureUsers = [
        {
          # Name must match the database for ensureDBOwnership, and the OS user
          # for socket peer authentication.
          name = "ravn";
          ensureDBOwnership = true;
        }
      ];
    };

    services.nats = mkIf cfg.nats.createLocally {
      enable = true;
      jetstream = true;
      settings = {
        host = "127.0.0.1";
        port = 4222;
      };
    };

    systemd.services.ravn-server = {
      description = "Ravn control plane";
      documentation = [ "https://github.com/olafkfreund/ravn-agents" ];
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ]
        ++ optional cfg.database.createLocally "postgresql.service"
        ++ optional cfg.nats.createLocally "nats.service";
      wants = [ "network-online.target" ];
      requires = optional cfg.database.createLocally "postgresql.service";

      environment = {
        RAVN_BIND = cfg.bind;
        DATABASE_URL = databaseUrl;
        NATS_URL = cfg.nats.url;
        RAVN_LOG = cfg.logLevel;
      } // optionalAttrs (cfg.oidc.issuer != null) {
        RAVN_OIDC_ISSUER = cfg.oidc.issuer;
      } // optionalAttrs (cfg.oidc.clientId != null) {
        RAVN_OIDC_CLIENT_ID = cfg.oidc.clientId;
      };

      serviceConfig = {
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";
        RestartSec = 5;

        # Fixed user so PostgreSQL peer auth maps to the `ravn` role.
        User = "ravn";
        Group = "ravn";
        StateDirectory = "ravn-server";

        # Hardening.
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectClock = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        ProtectProc = "invisible";
        RestrictSUIDSGID = true;
        RestrictRealtime = true;
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" ];
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" "~@resources" ];
        UMask = "0077";
      };
    };

    meta.maintainers = with lib.maintainers; [ ];
  };
}
