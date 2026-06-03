//! The control-plane's internal certificate authority (#19/#26).
//!
//! Loads a CA cert + key and signs agent CSRs into short-lived client
//! certificates. The issued certificate's identity is bound to the
//! *authenticated* `agent_id` from the enrollment request — the CSR's claimed
//! subject and SANs are discarded — so an agent cannot mint a certificate for
//! another identity.

use anyhow::{Context, Result};
use rcgen::{
    CertificateParams, CertificateSigningRequestParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Loaded CA material plus issuance policy.
pub struct Ca {
    issuer: rcgen::Certificate,
    key: KeyPair,
    cert_pem: String,
    ttl_days: i64,
}

impl Ca {
    /// Load a CA from PEM cert + key. `ttl_days` bounds issued certs.
    pub fn load(ca_cert_pem: &str, ca_key_pem: &str, ttl_days: i64) -> Result<Self> {
        let key = KeyPair::from_pem(ca_key_pem).context("parsing CA private key")?;
        let issuer = CertificateParams::from_ca_cert_pem(ca_cert_pem)
            .context("parsing CA certificate")?
            .self_signed(&key)
            .context("reconstructing CA issuer")?;
        Ok(Self { issuer, key, cert_pem: ca_cert_pem.trim().to_string(), ttl_days })
    }

    /// The CA certificate PEM, returned to agents so they can trust the chain.
    pub fn ca_cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Sign `csr_pem`, binding the certificate's identity to `agent_id`.
    /// Returns the client certificate PEM and its expiry.
    pub fn sign(&self, csr_pem: &str, agent_id: Uuid) -> Result<(String, OffsetDateTime)> {
        let mut csr =
            CertificateSigningRequestParams::from_pem(csr_pem).context("parsing CSR")?;

        // Override identity with the authenticated agent_id — never trust the
        // subject/SANs the CSR asked for.
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, agent_id.to_string());
        csr.params.distinguished_name = dn;
        let dns = format!("{agent_id}.agents.ravn");
        csr.params.subject_alt_names =
            vec![SanType::DnsName(dns.try_into().context("agent SAN")?)];

        // A client-auth leaf, never a CA.
        csr.params.is_ca = IsCa::ExplicitNoCa;
        csr.params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        csr.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

        let now = OffsetDateTime::now_utc();
        let not_after = now + Duration::days(self.ttl_days);
        csr.params.not_before = now - Duration::minutes(5); // tolerate clock skew
        csr.params.not_after = not_after;

        let cert = csr.signed_by(&self.issuer, &self.key).context("signing CSR")?;
        Ok((cert.pem(), not_after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::BasicConstraints;

    /// A throwaway CA for tests.
    fn test_ca() -> Ca {
        let key = KeyPair::generate().unwrap();
        let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.distinguished_name = DistinguishedName::new();
        p.distinguished_name.push(DnType::CommonName, "Ravn Test CA");
        let issuer = p.self_signed(&key).unwrap();
        let cert_pem = issuer.pem();
        Ca { issuer, key, cert_pem, ttl_days: 30 }
    }

    /// An agent-side CSR, as `ravn-agent` produces.
    fn agent_csr(claimed_cn: &str) -> String {
        let key = KeyPair::generate().unwrap();
        let mut p = CertificateParams::new(vec!["evil.example.com".to_string()]).unwrap();
        p.distinguished_name = DistinguishedName::new();
        p.distinguished_name.push(DnType::CommonName, claimed_cn);
        p.serialize_request(&key).unwrap().pem().unwrap()
    }

    #[test]
    fn signs_csr_into_client_cert() {
        let ca = test_ca();
        let agent_id = Uuid::now_v7();
        let (cert_pem, not_after) = ca.sign(&agent_csr("whatever"), agent_id).unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(not_after > OffsetDateTime::now_utc());

        // Issuer is the CA; subject identity is the agent_id, not the CSR claim.
        let pem = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes()).unwrap().1;
        let cert = pem.parse_x509().unwrap();
        assert!(cert.issuer().to_string().contains("Ravn Test CA"));
        assert!(cert.subject().to_string().contains(&agent_id.to_string()));
        // ClientAuth EKU present.
        let eku = cert.extended_key_usage().unwrap().unwrap().value;
        assert!(eku.client_auth);
    }

    #[test]
    fn identity_cannot_be_spoofed_via_csr_subject() {
        let ca = test_ca();
        let agent_id = Uuid::now_v7();
        // CSR claims a different identity; the signed cert must ignore it.
        let (cert_pem, _) = ca.sign(&agent_csr("11111111-1111-1111-1111-111111111111"), agent_id).unwrap();
        let pem = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes()).unwrap().1;
        let cert = pem.parse_x509().unwrap();
        assert!(cert.subject().to_string().contains(&agent_id.to_string()));
        assert!(!cert.subject().to_string().contains("11111111-1111"));
    }
}
