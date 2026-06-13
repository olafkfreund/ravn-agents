//! Startup configuration for the MCP server, from flags and environment.
//!
//! Precedence, per option: an explicit CLI flag wins over the environment
//! variable, which wins over the default. We hand-roll the parse (a handful of
//! flags) to keep the workspace dependency-light, matching the other crates.

/// Resolved configuration for one MCP server run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Control-plane base URL, e.g. `http://127.0.0.1:8080`.
    pub base_url: String,
    /// Bearer token presented to the control plane. Optional (auth may be off).
    pub token: Option<String>,
    /// Whether the gated mutating tools are exposed and callable.
    pub allow_mutations: bool,
    /// Tracing filter (`RUST_LOG`-style). Defaults to `warn` so protocol stdout
    /// stays clean — diagnostics go to stderr.
    pub log: String,
}

/// The default control-plane URL when none is configured.
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

impl Config {
    /// Parse from `args` (excluding the program name) and the process
    /// environment. Returns `Err` with a usage message on a bad flag, or
    /// `Ok(None)` when `--help` was requested.
    pub fn parse<I, S>(args: I) -> Result<Option<Self>, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());

        let mut base_url = env("RAVN_MCP_URL");
        let mut token = env("RAVN_MCP_TOKEN");
        // The env opt-in mirrors the flag: any truthy value enables mutations.
        let mut allow_mutations = env("RAVN_MCP_ALLOW_MUTATIONS")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let log = env("RAVN_MCP_LOG")
            .or_else(|| env("RUST_LOG"))
            .unwrap_or_else(|| "warn".into());

        let mut it = args.into_iter().map(|s| s.as_ref().to_string()).peekable();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--allow-mutations" => allow_mutations = true,
                "--url" => {
                    base_url = Some(it.next().ok_or_else(|| usage("--url needs a value"))?);
                }
                "--token" => {
                    token = Some(it.next().ok_or_else(|| usage("--token needs a value"))?);
                }
                other if other.starts_with("--url=") => {
                    base_url = Some(other.trim_start_matches("--url=").to_string());
                }
                other if other.starts_with("--token=") => {
                    token = Some(other.trim_start_matches("--token=").to_string());
                }
                other => return Err(usage(&format!("unknown argument: {other}"))),
            }
        }

        Ok(Some(Config {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            token,
            allow_mutations,
            log,
        }))
    }
}

/// The usage text, prefixed with an error line.
fn usage(error: &str) -> String {
    format!(
        "{error}\n\n{}",
        "ravn-mcp — the Ravn MCP server (read-only by default)\n\
         \n\
         USAGE:\n    \
             ravn-mcp [--url URL] [--token TOKEN] [--allow-mutations]\n\
         \n\
         OPTIONS:\n    \
             --url URL           Control-plane base URL (env RAVN_MCP_URL, default http://127.0.0.1:8080)\n    \
             --token TOKEN       Bearer token for the control plane (env RAVN_MCP_TOKEN)\n    \
             --allow-mutations   Expose approve/reject remediation tools (env RAVN_MCP_ALLOW_MUTATIONS).\n    \
             \x20                  Requires an admin-scoped --token. NEVER auto-executes.\n    \
             -h, --help          Print this help\n\
         \n\
         The server speaks MCP over stdio. Point an MCP client (e.g. Claude Code) at it."
    )
}

/// The bare help text (no error prefix), for `--help`.
pub fn help_text() -> String {
    usage("").trim_start().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The env vars are process-global; these tests avoid setting them and pass
    // only flags, so they don't race. Defaults are exercised via the no-flag
    // case being tolerant of ambient env (asserting only flag-driven fields).

    #[test]
    fn defaults_when_no_flags() {
        let cfg = Config::parse(Vec::<String>::new()).unwrap().unwrap();
        // base_url may come from ambient env in some shells; only assert the
        // mutation default, which no test env sets.
        assert!(!cfg.allow_mutations || std::env::var("RAVN_MCP_ALLOW_MUTATIONS").is_ok());
    }

    #[test]
    fn url_flag_space_form() {
        let cfg = Config::parse(["--url", "http://host:9000"])
            .unwrap()
            .unwrap();
        assert_eq!(cfg.base_url, "http://host:9000");
    }

    #[test]
    fn url_flag_equals_form() {
        let cfg = Config::parse(["--url=http://host:9000"]).unwrap().unwrap();
        assert_eq!(cfg.base_url, "http://host:9000");
    }

    #[test]
    fn token_flag() {
        let cfg = Config::parse(["--token", "secret"]).unwrap().unwrap();
        assert_eq!(cfg.token.as_deref(), Some("secret"));
    }

    #[test]
    fn allow_mutations_flag() {
        let cfg = Config::parse(["--allow-mutations"]).unwrap().unwrap();
        assert!(cfg.allow_mutations);
    }

    #[test]
    fn help_returns_none() {
        assert!(Config::parse(["--help"]).unwrap().is_none());
    }

    #[test]
    fn unknown_flag_errors() {
        assert!(Config::parse(["--nope"]).is_err());
    }

    #[test]
    fn missing_value_errors() {
        assert!(Config::parse(["--url"]).is_err());
    }
}
