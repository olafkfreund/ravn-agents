//! Command-envelope signing and verification (#114).
//!
//! The control plane signs each [`CommandEnvelope`] with an Ed25519 key; agents
//! and the actuator verify it against a pinned public key before executing. This
//! crate is the *only* place ed25519 is used — `ravn-core` stays crypto-free, and
//! every consumer depends on this crate rather than on ed25519 directly, so the
//! signing/verifying surface is small and auditable.
//!
//! The signed bytes are exactly [`CommandEnvelope::signing_payload`] (which
//! excludes the `sig` field). Verification also enforces expiry; replay defence
//! by nonce/`command_id` is the agent's responsibility (its idempotency ledger),
//! not this crate's.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, Verifier};
use rand::rngs::OsRng;
use ravn_core::CommandEnvelope;

// Re-exported so consumers (agent, actuator, server) name the key types through
// this crate and never take a direct ed25519 dependency. Also brings them into
// this module's scope for internal use.
pub use ed25519_dalek::{SigningKey, VerifyingKey};

/// Generate a fresh Ed25519 signing keypair (control-plane startup, #114 server side).
pub fn generate_signing_key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

/// Base64-encode a public key for delivery (e.g. in `EnrollResponse`).
pub fn verifying_key_to_b64(key: &VerifyingKey) -> String {
    B64.encode(key.to_bytes())
}

/// Decode a base64 public key pinned by an agent.
pub fn verifying_key_from_b64(s: &str) -> Result<VerifyingKey, KeyError> {
    let bytes = B64.decode(s.trim()).map_err(|_| KeyError::BadEncoding)?;
    let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| KeyError::BadLength)?;
    VerifyingKey::from_bytes(&arr).map_err(|_| KeyError::BadKey)
}

/// Base64-encode a private key for at-rest storage (`0600`, control plane only).
pub fn signing_key_to_b64(key: &SigningKey) -> String {
    B64.encode(key.to_bytes())
}

/// Decode a base64 private key loaded at startup.
pub fn signing_key_from_b64(s: &str) -> Result<SigningKey, KeyError> {
    let bytes = B64.decode(s.trim()).map_err(|_| KeyError::BadEncoding)?;
    let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| KeyError::BadLength)?;
    Ok(SigningKey::from_bytes(&arr))
}

/// Sign an envelope in place: sets `env.sig` to the base64 Ed25519 signature over
/// [`CommandEnvelope::signing_payload`].
pub fn sign_envelope(key: &SigningKey, env: &mut CommandEnvelope) {
    let sig = key.sign(&env.signing_payload());
    env.sig = Some(B64.encode(sig.to_bytes()));
}

/// Verify an envelope's signature and that it has not expired as of `now`.
///
/// Returns `Ok(())` only when the signature is present, decodes, validates
/// against `key` over the canonical payload, and `now <= expires_at`.
pub fn verify_envelope(
    key: &VerifyingKey,
    env: &CommandEnvelope,
    now: DateTime<Utc>,
) -> Result<(), VerifyError> {
    let sig_b64 = env.sig.as_deref().ok_or(VerifyError::MissingSignature)?;
    let sig_bytes = B64.decode(sig_b64.trim()).map_err(|_| VerifyError::BadEncoding)?;
    let sig_arr: [u8; 64] =
        sig_bytes.as_slice().try_into().map_err(|_| VerifyError::BadEncoding)?;
    let signature = Signature::from_bytes(&sig_arr);

    key.verify(&env.signing_payload(), &signature)
        .map_err(|_| VerifyError::BadSignature)?;

    if now > env.expires_at {
        return Err(VerifyError::Expired);
    }
    Ok(())
}

/// Failure decoding or constructing a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    BadEncoding,
    BadLength,
    BadKey,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::BadEncoding => write!(f, "key is not valid base64"),
            KeyError::BadLength => write!(f, "key has the wrong length"),
            KeyError::BadKey => write!(f, "key bytes are not a valid Ed25519 key"),
        }
    }
}

impl std::error::Error for KeyError {}

/// Why an envelope failed verification. The agent maps any of these to a
/// `Rejected` [`ravn_core::ActionResult`] — the command never executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// No signature on the envelope.
    MissingSignature,
    /// The signature was not valid base64 / wrong length.
    BadEncoding,
    /// The signature did not validate against the key and payload.
    BadSignature,
    /// The envelope's `expires_at` is in the past.
    Expired,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::MissingSignature => write!(f, "command envelope has no signature"),
            VerifyError::BadEncoding => write!(f, "command signature is malformed"),
            VerifyError::BadSignature => write!(f, "command signature does not verify"),
            VerifyError::Expired => write!(f, "command envelope has expired"),
        }
    }
}

impl std::error::Error for VerifyError {}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use ravn_core::{AgentId, ApprovalRef, Capability, CommandEnvelope, RiskTier};
    use uuid::Uuid;

    fn envelope(expires_at: DateTime<Utc>) -> CommandEnvelope {
        let now = Utc::now();
        CommandEnvelope {
            command_id: Uuid::now_v7(),
            agent_id: AgentId(Uuid::now_v7()),
            template_id: "failed-unit-restart".into(),
            template_version: 3,
            risk_tier: RiskTier::Safe,
            steps: vec![Capability::RestartUnit { unit: "nginx.service".into() }],
            verify: None,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "nonce-1".into(),
            issued_at: now,
            expires_at,
            sig: None,
        }
    }

    #[test]
    fn sign_then_verify_succeeds() {
        let key = generate_signing_key();
        let mut env = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&key, &mut env);
        assert!(env.sig.is_some());
        verify_envelope(&key.verifying_key(), &env, Utc::now()).unwrap();
    }

    #[test]
    fn tampered_payload_fails_verification() {
        let key = generate_signing_key();
        let mut env = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&key, &mut env);
        // Mutate a signed field after signing.
        env.steps = vec![Capability::RestartUnit { unit: "evil.service".into() }];
        assert_eq!(
            verify_envelope(&key.verifying_key(), &env, Utc::now()).unwrap_err(),
            VerifyError::BadSignature
        );
    }

    #[test]
    fn wrong_key_fails_verification() {
        let key = generate_signing_key();
        let other = generate_signing_key();
        let mut env = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&key, &mut env);
        assert_eq!(
            verify_envelope(&other.verifying_key(), &env, Utc::now()).unwrap_err(),
            VerifyError::BadSignature
        );
    }

    #[test]
    fn missing_signature_is_rejected() {
        let key = generate_signing_key();
        let env = envelope(Utc::now() + Duration::minutes(5));
        assert_eq!(
            verify_envelope(&key.verifying_key(), &env, Utc::now()).unwrap_err(),
            VerifyError::MissingSignature
        );
    }

    #[test]
    fn expired_envelope_is_rejected() {
        let key = generate_signing_key();
        let mut env = envelope(Utc::now() - Duration::minutes(1));
        sign_envelope(&key, &mut env);
        assert_eq!(
            verify_envelope(&key.verifying_key(), &env, Utc::now()).unwrap_err(),
            VerifyError::Expired
        );
    }

    #[test]
    fn public_key_b64_round_trips() {
        let key = generate_signing_key();
        let b64 = verifying_key_to_b64(&key.verifying_key());
        let back = verifying_key_from_b64(&b64).unwrap();
        assert_eq!(back, key.verifying_key());
    }

    #[test]
    fn signing_key_b64_round_trips_and_signs_compatibly() {
        let key = generate_signing_key();
        let b64 = signing_key_to_b64(&key);
        let back = signing_key_from_b64(&b64).unwrap();
        let mut env = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&back, &mut env);
        verify_envelope(&key.verifying_key(), &env, Utc::now()).unwrap();
    }
}
