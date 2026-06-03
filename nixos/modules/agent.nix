{ config, lib, pkgs, ... }:

let
  inherit (lib)
    mkEnableOption mkPackageOption mkOption mkIf mkDefault types
    optional optionalAttrs literalExpression;

  cfg = config.services.ravn.agent;

  settingsFormat = pkgs.formats.toml { };
  configFile = settingsFormat.generate "ravn-agent.toml" cfg.settings;

  # Resolve the GGUF model file from a path, or fetch a pinned URL into the store.
  inferenceModelFile =
    let m = cfg.inference.model;
    in
    if m.path != null then m.path
    else if m.url != null then "${pkgs.fetchurl { url = m.url; sha256 = m.sha256; }}"
    else null;

  inferenceEndpoint = "http://${cfg.inference.host}:${toString cfg.inference.port}";
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
          Enable local CPU inference for event explanations (epic #2). When a
          model is configured, runs `llama-server` as a sandboxed, loopback-only
          systemd unit in the resource-capped `ravn-inference` slice (#15).
        '';
      };

      package = mkPackageOption pkgs "llama-cpp" { };

      model = mkOption {
        default = { };
        description = ''
          The GGUF model to serve. Set `path` to a local file, or `url` + `sha256`
          to fetch a pinned model into the store. If neither is set, the
          llama-server unit is not created.
        '';
        type = types.submodule {
          options = {
            name = mkOption {
              type = types.str;
              default = "qwen3-1.7b-q4_k_m";
              description = "Model identifier reported on events and in config.";
            };
            path = mkOption {
              type = types.nullOr types.str;
              default = null;
              example = "/var/lib/ravn/models/qwen3-1.7b-q4_k_m.gguf";
              description = "Path to a GGUF model file on the host.";
            };
            url = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "URL of a GGUF model to fetch (pin with `sha256`).";
            };
            sha256 = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "Hash for `url` (required when `url` is set).";
            };
          };
        };
      };

      host = mkOption {
        type = types.str;
        default = "127.0.0.1";
        description = "Address llama-server binds to (loopback by default).";
      };
      port = mkOption {
        type = types.port;
        default = 18181;
        description = "Port llama-server listens on (loopback-only).";
      };
      contextSize = mkOption {
        type = types.ints.positive;
        default = 4096;
        description = "Model context window (`--ctx-size`).";
      };
      threads = mkOption {
        type = types.ints.unsigned;
        default = 0;
        description = "Inference threads (`--threads`); 0 lets llama.cpp choose.";
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
        model = mkDefault cfg.inference.model.name;
        endpoint = mkDefault inferenceEndpoint;
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

    # llama-server (#15): sandboxed, loopback-only, in the inference slice.
    # Only created once a model is configured.
    systemd.services.ravn-llama = mkIf (cfg.inference.enable && inferenceModelFile != null) {
      description = "Ravn local inference (llama-server)";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      serviceConfig = {
        ExecStart = lib.concatStringsSep " " ([
          "${cfg.inference.package}/bin/llama-server"
          "--model ${inferenceModelFile}"
          "--host ${cfg.inference.host}"
          "--port ${toString cfg.inference.port}"
          "--ctx-size ${toString cfg.inference.contextSize}"
        ] ++ optional (cfg.inference.threads > 0) "--threads ${toString cfg.inference.threads}");
        Restart = "on-failure";
        RestartSec = 5;
        Slice = "ravn-inference.slice";

        DynamicUser = true;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        ProtectProc = "invisible";
        RestrictSUIDSGID = true;
        RestrictRealtime = true;
        RestrictNamespaces = true;
        LockPersonality = true;
        # ggml may allocate executable memory — keep W^X relaxed for inference only.
        MemoryDenyWriteExecute = false;
        # Loopback-only: bind 127.0.0.1 and refuse non-local traffic.
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        IPAddressAllow = "localhost";
        IPAddressDeny = "any";
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" ];
        UMask = "0077";
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
