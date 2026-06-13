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

use std::collections::BTreeMap;

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

/// Stable identifier for a signing key: the first 16 base64 chars of the
/// public key. Deterministic from the key alone, so the control plane and every
/// agent derive the same `kid` for the same key without coordination, and a
/// rotated-in key gets a distinct id automatically.
pub fn key_id(key: &VerifyingKey) -> String {
    let mut id = B64.encode(key.to_bytes());
    id.truncate(16);
    id
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

/// Sign an envelope in place: stamps `env.kid` with the signing key's id and sets
/// `env.sig` to the base64 Ed25519 signature over [`CommandEnvelope::signing_payload`].
/// The `kid` is set *before* signing, so it is covered by the signature.
pub fn sign_envelope(key: &SigningKey, env: &mut CommandEnvelope) {
    env.kid = Some(key_id(&key.verifying_key()));
    let sig = key.sign(&env.signing_payload());
    env.sig = Some(B64.encode(sig.to_bytes()));
}

/// Verify an envelope's signature and that it has not expired as of `now`.
///
/// Returns `Ok(())` only when the signature is present, decodes, validates
/// against `key` over the canonical payload, and `now <= expires_at`.
///
/// Single-key convenience: rotation-aware verifiers (agent, actuator) use a
/// [`Keyring`] via [`verify_envelope_with`], which selects the key by `kid`.
pub fn verify_envelope(
    key: &VerifyingKey,
    env: &CommandEnvelope,
    now: DateTime<Utc>,
) -> Result<(), VerifyError> {
    verify_signature(key, env)?;
    if now > env.expires_at {
        return Err(VerifyError::Expired);
    }
    Ok(())
}

/// Verify an envelope against a keyring (#150): select the trusted key by the
/// envelope's `kid`, validate the signature and expiry. This is what makes
/// zero-downtime rotation work — during the overlap window the keyring holds
/// both the old and new keys, so commands signed by either verify.
///
/// `kid` resolution:
/// - `Some(kid)` → the key with that id must be in the ring, or [`VerifyError::UnknownKey`].
/// - `None` (pre-rotation envelope) → the ring's default key (the legacy
///   single-pinned key), so old envelopes keep verifying.
pub fn verify_envelope_with(
    ring: &Keyring,
    env: &CommandEnvelope,
    now: DateTime<Utc>,
) -> Result<(), VerifyError> {
    let key = match env.kid.as_deref() {
        Some(kid) => ring.get(kid).ok_or(VerifyError::UnknownKey)?,
        None => ring.default_key().ok_or(VerifyError::UnknownKey)?,
    };
    verify_signature(key, env)?;
    if now > env.expires_at {
        return Err(VerifyError::Expired);
    }
    Ok(())
}

/// Validate just the Ed25519 signature against `key` (no expiry check).
fn verify_signature(key: &VerifyingKey, env: &CommandEnvelope) -> Result<(), VerifyError> {
    let sig_b64 = env.sig.as_deref().ok_or(VerifyError::MissingSignature)?;
    let sig_bytes = B64.decode(sig_b64.trim()).map_err(|_| VerifyError::BadEncoding)?;
    let sig_arr: [u8; 64] =
        sig_bytes.as_slice().try_into().map_err(|_| VerifyError::BadEncoding)?;
    let signature = Signature::from_bytes(&sig_arr);
    key.verify(&env.signing_payload(), &signature)
        .map_err(|_| VerifyError::BadSignature)
}

/// A set of trusted command-signing public keys, indexed by `kid` (#150).
///
/// Agents and the actuator verify against a keyring rather than a single key, so
/// a key can be rotated with zero command-verification failures: publish the new
/// key alongside the old, let agents fetch it on their next check-in, then retire
/// the old key after an overlap window — no fleet re-enrollment.
///
/// One key is the **default**: it verifies legacy envelopes that carry no `kid`
/// (minted before rotation). It is also, by convention, the key the control plane
/// is currently signing with. Serializes to/from a small, authenticated-channel
/// JSON document via [`Keyring::to_json`] / [`Keyring::from_json`].
#[derive(Debug, Clone, Default)]
pub struct Keyring {
    keys: BTreeMap<String, VerifyingKey>,
    default_kid: Option<String>,
}

impl Keyring {
    /// An empty keyring (no trusted keys). Verification always fails until at
    /// least one key is added — fail closed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a keyring from a single key, which becomes the default. Equivalent
    /// to the pre-rotation single-pinned-key behaviour.
    pub fn single(key: VerifyingKey) -> Self {
        let kid = key_id(&key);
        let mut keys = BTreeMap::new();
        keys.insert(kid.clone(), key);
        Self { keys, default_kid: Some(kid) }
    }

    /// Add (or replace) a trusted key, returning its `kid`. Does not change which
    /// key is the default.
    pub fn insert(&mut self, key: VerifyingKey) -> String {
        let kid = key_id(&key);
        self.keys.insert(kid.clone(), key);
        kid
    }

    /// Mark `kid` as the default key (used to verify envelopes that carry no
    /// `kid`, and advertised as the active signing key). The key must already be
    /// present; returns `false` if it is not.
    pub fn set_default(&mut self, kid: &str) -> bool {
        if self.keys.contains_key(kid) {
            self.default_kid = Some(kid.to_string());
            true
        } else {
            false
        }
    }

    /// Look up a trusted key by `kid`.
    pub fn get(&self, kid: &str) -> Option<&VerifyingKey> {
        self.keys.get(kid)
    }

    /// The default key, if one is set.
    pub fn default_key(&self) -> Option<&VerifyingKey> {
        self.default_kid.as_deref().and_then(|k| self.keys.get(k))
    }

    /// The default key's id, if one is set.
    pub fn default_kid(&self) -> Option<&str> {
        self.default_kid.as_deref()
    }

    /// Number of trusted keys in the ring.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the ring holds no keys.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// All `kid`s in the ring, sorted.
    pub fn kids(&self) -> Vec<String> {
        self.keys.keys().cloned().collect()
    }

    /// Serialize to the wire/disk JSON document the control plane publishes and
    /// agents pin: `{ "active_kid": "...", "keys": { kid: pubkey_b64, ... } }`.
    pub fn to_json(&self) -> String {
        let keys: BTreeMap<&String, String> =
            self.keys.iter().map(|(k, v)| (k, verifying_key_to_b64(v))).collect();
        let doc = serde_json::json!({
            "active_kid": self.default_kid,
            "keys": keys,
        });
        serde_json::to_string(&doc).expect("keyring document always serializes")
    }

    /// Parse a keyring from its JSON document. Rejects malformed keys
    /// (fail closed) and an `active_kid` that is not among the keys.
    pub fn from_json(s: &str) -> Result<Self, KeyError> {
        #[derive(serde::Deserialize)]
        struct Doc {
            active_kid: Option<String>,
            keys: BTreeMap<String, String>,
        }
        let doc: Doc = serde_json::from_str(s).map_err(|_| KeyError::BadEncoding)?;
        let mut keys = BTreeMap::new();
        for (kid, b64) in doc.keys {
            keys.insert(kid, verifying_key_from_b64(&b64)?);
        }
        let default_kid = match doc.active_kid {
            Some(k) if keys.contains_key(&k) => Some(k),
            Some(_) => return Err(KeyError::BadKey),
            None => None,
        };
        Ok(Self { keys, default_kid })
    }
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
    /// The envelope's `kid` is not in the verifier's keyring — either a forged
    /// id, or a key that was retired (#150). Treated like a bad signature: the
    /// command never executes.
    UnknownKey,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::MissingSignature => write!(f, "command envelope has no signature"),
            VerifyError::BadEncoding => write!(f, "command signature is malformed"),
            VerifyError::BadSignature => write!(f, "command signature does not verify"),
            VerifyError::Expired => write!(f, "command envelope has expired"),
            VerifyError::UnknownKey => write!(f, "command signed by an untrusted or retired key"),
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
            preconditions: vec![],
            steps: vec![Capability::RestartUnit { unit: "nginx.service".into() }],
            verify: None,
            rollback: ravn_core::Rollback::None,
            approval_ref: ApprovalRef::PolicyAuto,
            nonce: "nonce-1".into(),
            issued_at: now,
            expires_at,
            kid: None,
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

    #[test]
    fn signing_stamps_the_key_id() {
        let key = generate_signing_key();
        let mut env = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&key, &mut env);
        assert_eq!(env.kid.as_deref(), Some(key_id(&key.verifying_key()).as_str()));
    }

    #[test]
    fn key_id_is_stable_and_distinct_per_key() {
        let a = generate_signing_key();
        let b = generate_signing_key();
        assert_eq!(key_id(&a.verifying_key()), key_id(&a.verifying_key()));
        assert_ne!(key_id(&a.verifying_key()), key_id(&b.verifying_key()));
    }

    // The core #150 property: during the overlap window the keyring holds BOTH
    // the old and the new key, so a command signed by EITHER verifies — zero
    // command-verification failures while the fleet migrates.
    #[test]
    fn keyring_verifies_across_two_valid_keys_during_overlap() {
        let old = generate_signing_key();
        let new = generate_signing_key();
        let mut ring = Keyring::single(old.verifying_key());
        ring.insert(new.verifying_key());

        let mut env_old = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&old, &mut env_old);
        let mut env_new = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&new, &mut env_new);

        verify_envelope_with(&ring, &env_old, Utc::now()).unwrap();
        verify_envelope_with(&ring, &env_new, Utc::now()).unwrap();
    }

    // After the overlap window the old key is retired (dropped from the ring);
    // commands it signed are then rejected as UnknownKey.
    #[test]
    fn keyring_rejects_a_retired_key() {
        let old = generate_signing_key();
        let new = generate_signing_key();
        let mut env_old = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&old, &mut env_old);

        // Retired ring: only the new key remains.
        let ring = Keyring::single(new.verifying_key());
        assert_eq!(
            verify_envelope_with(&ring, &env_old, Utc::now()).unwrap_err(),
            VerifyError::UnknownKey
        );
    }

    // A pre-rotation envelope (no `kid`) verifies against the ring's default key,
    // so existing in-flight commands keep working when rotation ships.
    #[test]
    fn keyring_verifies_legacy_envelope_without_kid_via_default() {
        let key = generate_signing_key();
        let mut env = envelope(Utc::now() + Duration::minutes(5));
        // Sign the old way (no kid stamped), as a pre-#150 control plane would.
        let sig = key.sign(&env.signing_payload());
        env.sig = Some(B64.encode(sig.to_bytes()));
        assert!(env.kid.is_none());

        let ring = Keyring::single(key.verifying_key());
        verify_envelope_with(&ring, &env, Utc::now()).unwrap();
    }

    #[test]
    fn keyring_rejects_forged_kid_pointing_at_trusted_key() {
        // Envelope signed by an attacker key, but its `kid` claims a trusted one.
        let trusted = generate_signing_key();
        let attacker = generate_signing_key();
        let ring = Keyring::single(trusted.verifying_key());

        let mut env = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&attacker, &mut env);
        env.kid = Some(key_id(&trusted.verifying_key())); // lie about the key id
        assert_eq!(
            verify_envelope_with(&ring, &env, Utc::now()).unwrap_err(),
            VerifyError::BadSignature
        );
    }

    #[test]
    fn empty_keyring_fails_closed() {
        let key = generate_signing_key();
        let mut env = envelope(Utc::now() + Duration::minutes(5));
        sign_envelope(&key, &mut env);
        let ring = Keyring::new();
        assert_eq!(
            verify_envelope_with(&ring, &env, Utc::now()).unwrap_err(),
            VerifyError::UnknownKey
        );
    }

    #[test]
    fn keyring_json_round_trips() {
        let old = generate_signing_key();
        let new = generate_signing_key();
        let mut ring = Keyring::single(old.verifying_key());
        let new_kid = ring.insert(new.verifying_key());
        // Advertise the new key as active (the rotation step).
        assert!(ring.set_default(&new_kid));

        let json = ring.to_json();
        let back = Keyring::from_json(&json).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back.default_kid(), Some(new_kid.as_str()));
        assert_eq!(back.get(&new_kid), Some(&new.verifying_key()));
        assert_eq!(back.get(&key_id(&old.verifying_key())), Some(&old.verifying_key()));
    }

    #[test]
    fn keyring_json_rejects_active_kid_not_in_keys() {
        let json = r#"{"active_kid":"deadbeef","keys":{}}"#;
        assert_eq!(Keyring::from_json(json).unwrap_err(), KeyError::BadKey);
    }

    #[test]
    fn keyring_expired_envelope_is_rejected() {
        let key = generate_signing_key();
        let mut env = envelope(Utc::now() - Duration::minutes(1));
        sign_envelope(&key, &mut env);
        let ring = Keyring::single(key.verifying_key());
        assert_eq!(
            verify_envelope_with(&ring, &env, Utc::now()).unwrap_err(),
            VerifyError::Expired
        );
    }
}
