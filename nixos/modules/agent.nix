{ config, lib, pkgs, ... }:

let
  inherit (lib)
    mkEnableOption mkPackageOption mkOption mkIf mkDefault types
    optional optionalAttrs literalExpression;

  cfg = config.services.ravn.agent;

  settingsFormat = pkgs.formats.toml { };
  configFile = settingsFormat.generate "ravn-agent.toml" cfg.settings;
in
{
  options.services.ravn.agent = {
    enable = mkEnableOption "the Ravn detection agent (ravnd)";

    package = mkPackageOption pkgs "ravn-agent" { };

    server.url = mkOption {
      type = types.str;
      example = "nats://control.example.com:4222";
      description = ''
        Control-plane transport endpoint the agent connects to. NATS in normal
        operation; a `ws://` URL is accepted for the M0 WebSocket fallback (#22).
      '';
    };

    enrollment.bootstrapTokenFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/run/secrets/ravn-bootstrap-token";
      description = ''
        Path to a file containing the bootstrap token used for first-time
        enrollment (#19). It is passed to the service via systemd credentials
        and read at runtime — never copied into the world-readable Nix store.
      '';
    };

    detection = {
      journald.enable = mkOption {
        type = types.bool;
        default = true;
        description = "Read structured events from the systemd journal (#9).";
      };
      failedUnits.enable = mkOption {
        type = types.bool;
        default = true;
        description = "Detect units entering a failed state over D-Bus (#10).";
      };
      configDrift.paths = mkOption {
        type = types.listOf types.path;
        default = [ ];
        example = literalExpression ''[ "/etc/nixos" "/etc/ssh/sshd_config" ]'';
        description = "Paths watched for content drift via inotify + hashing (#11).";
      };
      auth.enable = mkOption {
        type = types.bool;
        default = true;
        description = "Extract SSH/auth/audit events from the journal (#12).";
      };
      updates.enable = mkOption {
        type = types.bool;
        default = true;
        description = "Detect system updates / NixOS generation changes (#13).";
      };
    };

    inference = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Enable local CPU inference for event explanations (epic #2). The
          `llama-server` unit itself is provided by a separate module (#15);
          this only wires the agent to use it and sizes its resource slice.
        '';
      };
      model = mkOption {
        type = types.str;
        default = "qwen3-1.7b-q4_k_m";
        description = "Identifier of the pinned local model to use.";
      };
      cpuQuota = mkOption {
        type = types.str;
        default = "200%";
        example = "400%";
        description = "`CPUQuota` applied to the inference slice (#18).";
      };
      memoryMax = mkOption {
        type = types.str;
        default = "2G";
        description = "`MemoryMax` applied to the inference slice (#18).";
      };
    };

    logLevel = mkOption {
      type = types.enum [ "error" "warn" "info" "debug" "trace" ];
      default = "info";
      description = "Log verbosity for the agent.";
    };

    settings = mkOption {
      type = types.submodule { freeformType = settingsFormat.type; };
      default = { };
      description = ''
        Free-form settings written verbatim to the agent's TOML config. Options
        above populate sensible defaults; anything set here overrides them.
        Must not contain secrets — the config lands in the Nix store.
      '';
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.server.url != "";
        message = "services.ravn.agent.server.url must be set.";
      }
    ];

    # Translate the typed options into the agent's config file.
    services.ravn.agent.settings = {
      server.url = mkDefault cfg.server.url;
      log.level = mkDefault cfg.logLevel;
      detection = {
        journald.enable = mkDefault cfg.detection.journald.enable;
        failed_units.enable = mkDefault cfg.detection.failedUnits.enable;
        config_drift.paths = mkDefault cfg.detection.configDrift.paths;
        auth.enable = mkDefault cfg.detection.auth.enable;
        updates.enable = mkDefault cfg.detection.updates.enable;
      };
      inference = mkIf cfg.inference.enable {
        enable = mkDefault true;
        model = mkDefault cfg.inference.model;
      };
    };

    # Dedicated, resource-capped slice for local inference (#18). The
    # llama-server unit (#15) joins this slice; sizing it here keeps the
    # inference workload from starving the host.
    systemd.slices."ravn-inference" = mkIf cfg.inference.enable {
      description = "Ravn local inference";
      sliceConfig = {
        CPUQuota = cfg.inference.cpuQuota;
        MemoryMax = cfg.inference.memoryMax;
      };
    };

    systemd.services.ravnd = {
      description = "Ravn detection agent";
      documentation = [ "https://github.com/olafkfreund/ravn-agents" ];
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} --config ${configFile}";
        Restart = "on-failure";
        RestartSec = 5;

        # Identity & state.
        DynamicUser = true;
        # Read the journal for the journald and auth/SSH taps (#9, #12).
        SupplementaryGroups = [ "systemd-journal" ];
        # Local SQLite offline buffer lives here (#21): /var/lib/ravn-agent.
        StateDirectory = "ravn-agent";
        StateDirectoryMode = "0700";

        # Bootstrap token delivered as a credential, not via the store/env.
        LoadCredential =
          optional (cfg.enrollment.bootstrapTokenFile != null)
            "bootstrap-token:${cfg.enrollment.bootstrapTokenFile}";

        # Hardening. A read-only view of the system is exactly what the taps
        # need, so `strict` costs us nothing.
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
