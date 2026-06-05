//! Agent enrollment client (#19).
//!
//! On startup the agent ensures it holds an mTLS identity: if credentials are
//! already on disk it reuses them (idempotent — no control-plane round trip),
//! otherwise — given a bootstrap token and enrollment endpoint — it generates a
//! keypair + CSR, exchanges the token at `/enroll`, and persists the signed
//! certificate, private key, and CA. Enrollment never blocks detection: a
//! failure is logged and the agent continues without an identity.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use ravn_core::{AgentId, EnrollRequest, EnrollResponse};
use uuid::Uuid;

use crate::config::Config;

/// On-disk locations of the agent's mTLS credentials.
#[derive(Debug, Clone)]
pub struct Identity {
    pub key_path: PathBuf,
    pub cert_path: PathBuf,
    pub ca_path: PathBuf,
}

impl Identity {
    fn in_dir(dir: &Path) -> Self {
        Self {
            key_path: dir.join("agent.key"),
            cert_path: dir.join("agent.crt"),
            ca_path: dir.join("ca.crt"),
        }
    }

    /// All three credential files are present.
    fn complete(&self) -> bool {
        self.key_path.exists() && self.cert_path.exists() && self.ca_path.exists()
    }
}

/// Ensure the agent holds an mTLS identity, enrolling if necessary.
///
/// Returns `Some` when an identity is available (reused or freshly issued) and
/// `None` when enrollment isn't configured.
pub async fn ensure_enrolled(config: &Config) -> Result<Option<Identity>> {
    let id = Identity::in_dir(&config.cred_dir);

    if id.complete() {
        tracing::info!(dir = %config.cred_dir.display(), "agent credentials present; reusing");
        return Ok(Some(id));
    }

    let (Some(endpoint), Some(token)) =
        (config.enroll_endpoint.as_deref(), config.bootstrap_token.as_deref())
    else {
        tracing::info!("enrollment not configured; continuing without an mTLS identity");
        return Ok(None);
    };

    let (csr_pem, key_pem) = generate_csr(config.agent_id)?;

    let url = format!("{}/enroll", endpoint.trim_end_matches('/'));
    let request = EnrollRequest {
        bootstrap_token: token.to_string(),
        agent_id: AgentId(config.agent_id),
        host: config.host.clone(),
        csr_pem,
    };
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("building enrollment HTTP client")?;
    let resp = http.post(&url).json(&request).send().await.with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("enrollment rejected ({status}): {body}");
    }
    let enrolled: EnrollResponse = resp.json().await.context("decoding enrollment response")?;

    std::fs::create_dir_all(&config.cred_dir)
        .with_context(|| format!("creating credential dir {}", config.cred_dir.display()))?;
    write_private(&id.key_path, &key_pem)?;
    std::fs::write(&id.cert_path, &enrolled.certificate_pem)
        .with_context(|| format!("writing {}", id.cert_path.display()))?;
    std::fs::write(&id.ca_path, &enrolled.ca_certificate_pem)
        .with_context(|| format!("writing {}", id.ca_path.display()))?;
    // Pin the control plane's command-signing public key (#114): the agent
    // rejects any remediation command that does not verify against it.
    std::fs::write(config.cred_dir.join("command_pubkey.b64"), &enrolled.command_signing_pubkey)
        .with_context(|| format!("writing command_pubkey.b64 in {}", config.cred_dir.display()))?;

    tracing::info!(
        agent_id = %config.agent_id,
        not_after = %enrolled.not_after,
        dir = %config.cred_dir.display(),
        "enrolled; stored mTLS credentials"
    );
    Ok(Some(id))
}

/// Generate an agent keypair and a PKCS#10 CSR bound to `agent_id`. Returns
/// `(csr_pem, key_pem)`. The control plane re-binds the identity at signing, so
/// the subject here is advisory.
fn generate_csr(agent_id: Uuid) -> Result<(String, String)> {
    let key_pair = KeyPair::generate().context("generating agent keypair")?;
    let mut params = CertificateParams::new(vec![format!("{agent_id}.agents.ravn")])
        .context("building CSR parameters")?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, agent_id.to_string());
    params.distinguished_name = dn;
    let csr_pem = params
        .serialize_request(&key_pair)
        .context("serializing CSR")?
        .pem()
        .context("encoding CSR as PEM")?;
    Ok((csr_pem, key_pair.serialize_pem()))
}

/// Write a file with owner-only (0600) permissions.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    f.write_all(contents.as_bytes()).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_csr_is_valid_pem_and_parses() {
        let agent_id = Uuid::now_v7();
        let (csr_pem, key_pem) = generate_csr(agent_id).unwrap();
        assert!(csr_pem.contains("BEGIN CERTIFICATE REQUEST"));
        assert!(key_pem.contains("BEGIN PRIVATE KEY"));
        assert!(csr_pem.contains("END CERTIFICATE REQUEST"));
        // (The server actually parsing/signing this CSR is covered by the live
        // enrollment e2e, against the real /enroll endpoint.)
    }

    #[test]
    fn complete_requires_all_three_files() {
        let dir = std::env::temp_dir().join(format!("ravn-enroll-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = Identity::in_dir(&dir);
        assert!(!id.complete());
        std::fs::write(&id.key_path, "k").unwrap();
        std::fs::write(&id.cert_path, "c").unwrap();
        assert!(!id.complete(), "missing ca.crt should not count as complete");
        std::fs::write(&id.ca_path, "a").unwrap();
        assert!(id.complete());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
