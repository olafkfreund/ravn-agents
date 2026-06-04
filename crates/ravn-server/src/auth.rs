//! In-cluster agent authentication (#57): validate a Kubernetes projected
//! ServiceAccount token (a JWT) against the cluster's OIDC JWKS.
//!
//! The default path is OIDC/JWKS — the control plane is configured with the
//! cluster's issuer + JWKS and verifies the token's RS256 signature, issuer,
//! audience, and expiry locally (no per-request call to the API server). The
//! Kubernetes `TokenReview` fallback named in #57 is a follow-up (see the PR);
//! this delivers the default.
//!
//! Used only by the authenticated HTTP ingest endpoint, so in-cluster agents
//! (the controller #55 and node-agent #56) can present their projected token
//! instead of publishing to an unauthenticated NATS subject.

use anyhow::{anyhow, Context};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

/// Claims we read off a validated ServiceAccount token. The `sub` is the
/// agent's identity, e.g. `system:serviceaccount:ravn:ravn-controller`.
#[derive(Debug, Clone, Deserialize)]
pub struct SaClaims {
    pub sub: String,
}

/// Validates ServiceAccount JWTs against a fixed issuer/audience and JWKS.
pub struct IngestAuth {
    issuer: String,
    audience: String,
    jwks: JwkSet,
}

impl IngestAuth {
    /// Build a validator from an issuer, expected audience, and the JWKS JSON
    /// document served at the issuer's `jwks_uri`.
    pub fn new(issuer: String, audience: String, jwks_json: &str) -> anyhow::Result<Self> {
        let jwks: JwkSet = serde_json::from_str(jwks_json).context("parsing OIDC JWKS")?;
        if jwks.keys.is_empty() {
            return Err(anyhow!("OIDC JWKS contains no keys"));
        }
        Ok(Self { issuer, audience, jwks })
    }

    /// Verify a bearer token's signature, issuer, audience, and expiry,
    /// returning its claims. Any failure is an authentication error.
    pub fn validate(&self, token: &str) -> anyhow::Result<SaClaims> {
        let header = decode_header(token).context("decoding token header")?;
        let kid = header.kid.context("token has no `kid`")?;
        let jwk = self
            .jwks
            .find(&kid)
            .ok_or_else(|| anyhow!("no JWKS key matches token `kid` {kid}"))?;
        let key = DecodingKey::from_jwk(jwk).context("building decoding key from JWK")?;

        // ServiceAccount tokens are RS256; pin it rather than trusting the
        // header's `alg` (avoids algorithm-confusion attacks).
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);

        let data =
            decode::<SaClaims>(token, &key, &validation).context("verifying token signature/claims")?;
        Ok(data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;

    // A throwaway RSA-2048 keypair generated for tests only, with its matching
    // JWK (kid `ravn-test-key`). Lets us exercise the real from_jwk + RS256
    // verification path without a live cluster.
    const TEST_RSA_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDYaO8r985WyLdv
uELiEH0FPTsZ+QbSXul5+u3UUMGhf0s84JU4BZqw0xMcsXYAj8mnM3G6Lj8dLrqF
1qotF4w8gk2sJyYNgxRGwGKB6iLPUfr8l7coPU/lb+ZjqR6S/GJKNmXjDQ8hS1S/
ebKO6Mvm+kRav83di9kjpNyBKHwylVDl2fDFWow5ELeXrUYG0TRaNX2hLi+tG0Yz
1mvDVejQf5l3tUAN53+05ODqr9eI5hz+1jkYULZxpGer1ohzhHQL/QIosqp5ZgJx
lkymGxmvMdYOl6XxhPZVxnU5sAaWgUSkClbpLn5eHYY41HpGH1B74D+E8aCovtlR
dO+wjfG7AgMBAAECggEAJv6ZgiGv44FdVFsuag+wh14mJSLzMLr7dQhbDHPUwCXw
x7JsEOEpo40VF4l+itFd86vYZUTqCHcgEvfASEnC8jBEkK2pNKwW3jzSQziONy89
e4BW94A8wknsiK6znKavi1HMACKdRFGPnsTuAMQ/4YndAUEodjA52ytctEU4Q+DB
CAWKwb03HUL2pkDdaYqwetVyswdi6/v2jhWy6hUxuOQLx31CESM6ZpCBW6/NT8Vw
Otq8Q3/Mu8E9q0MUv5P7PLG0vqFcO1gJ+myje65K0R/Lm+VNyFfy1H49l6CF9qwp
ISSpgK6Un70T5MTVzRE1DNaV1w4CeHCCkjgytk0h4QKBgQDznYs6XizzgK3c7YHR
D2Pgru6hbVVMTHyI1FzLsn/NK1dQBm8sm4Hpg7CuHCRYPDO9DFAvvoMKirsc6twg
lDS7RiTOGm5w3K3ixzJyOFlZCbUIukrfxiF3r0aV1whk8OpsIEt5NjjNepTd7jyB
X5RM6uz5ziX2kZ/gscNFOQsXMQKBgQDjaVU4iEKKgrVzYlwcRX0btK/QuW1zgkoC
SZiaiuIfPe4Q3znFsxmzYDRlLubfJiLoQTH3F67W/JGM58QlAHw1tl4OzhMVSR8c
VrhFVu8ziGdNirx31Nq42OZz7bKLZou6op6/D190Ak1JzHP5SvqRfluusEYMlk73
hilwv4e0qwKBgQCUL9v2GD0trbOUtOCHg94UWTSM+02siMYkEVGVErJM4jVNV2ye
7MUcf1+kuTeeeJhcQbYxJKjfa74f+/kE1EIzPJq8yDUv7/zR+quD8STgVVhKw88x
yXqoK/U6xj+z7xwZw5dFVyc8TnlpejZR2AsEss6NsclD8BcZfegzHlzRsQKBgQDQ
e5anVzQ5q48SExCC0qnZppKwde6DwOR8qGAA/mZDYhFI4n0iZAmhywb95DvAREQo
TOyzrMCbU71UQn4ttf4pd+FPDVmtX/XnkxEocISm59xc2F3kNf23DRJpIXdYGVDs
b329hyhpQFr+1zNTTovcqsz+n5f4niwS/KotNUoCNQKBgBZcNliEA4tfjSxiov12
HiyOQlqliSMuvGZnOb+DG1hBKJYsESsE7BC06koPiUn574RgBR3fv8kPUPXaG7Ia
4xHHMbSj2p0bdc8ZqryAp/QdB0A9hlipIZM+v2Ur4o7clhwMmA9hM8n2VPkWGZbo
fv0uzdhEI+5TFOnO7jqhncO6
-----END PRIVATE KEY-----";

    const TEST_JWK_N: &str = "2GjvK_fOVsi3b7hC4hB9BT07GfkG0l7pefrt1FDBoX9LPOCVOAWasNMTHLF2AI_JpzNxui4_HS66hdaqLReMPIJNrCcmDYMURsBigeoiz1H6_Je3KD1P5W_mY6kekvxiSjZl4w0PIUtUv3myjujL5vpEWr_N3YvZI6TcgSh8MpVQ5dnwxVqMORC3l61GBtE0WjV9oS4vrRtGM9Zrw1Xo0H-Zd7VADed_tOTg6q_XiOYc_tY5GFC2caRnq9aIc4R0C_0CKLKqeWYCcZZMphsZrzHWDpel8YT2VcZ1ObAGloFEpApW6S5-Xh2GONR6Rh9Qe-A_hPGgqL7ZUXTvsI3xuw";

    fn test_jwks() -> String {
        format!(
            r#"{{"keys":[{{"kty":"RSA","use":"sig","kid":"ravn-test-key","alg":"RS256","n":"{TEST_JWK_N}","e":"AQAB"}}]}}"#
        )
    }

    #[derive(Serialize)]
    struct Claims {
        sub: String,
        iss: String,
        aud: String,
        exp: usize,
    }

    fn sign(claims: &Claims) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("ravn-test-key".to_string());
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).unwrap();
        encode(&header, claims, &key).unwrap()
    }

    fn claims(iss: &str, aud: &str, exp_offset: i64) -> Claims {
        // Fixed base time; we only need exp relative to "now" via a large value.
        let exp = (chrono::Utc::now().timestamp() + exp_offset) as usize;
        Claims {
            sub: "system:serviceaccount:ravn:ravn-controller".into(),
            iss: iss.into(),
            aud: aud.into(),
            exp,
        }
    }

    fn auth() -> IngestAuth {
        IngestAuth::new("https://kube.test".into(), "ravn".into(), &test_jwks()).unwrap()
    }

    #[test]
    fn accepts_a_valid_serviceaccount_token() {
        let token = sign(&claims("https://kube.test", "ravn", 600));
        let c = auth().validate(&token).expect("a valid token must pass");
        assert_eq!(c.sub, "system:serviceaccount:ravn:ravn-controller");
    }

    #[test]
    fn rejects_wrong_audience() {
        let token = sign(&claims("https://kube.test", "not-ravn", 600));
        assert!(auth().validate(&token).is_err());
    }

    #[test]
    fn rejects_wrong_issuer() {
        let token = sign(&claims("https://evil.test", "ravn", 600));
        assert!(auth().validate(&token).is_err());
    }

    #[test]
    fn rejects_expired_token() {
        // Beyond jsonwebtoken's default 60s clock-skew leeway.
        let token = sign(&claims("https://kube.test", "ravn", -120));
        assert!(auth().validate(&token).is_err());
    }

    #[test]
    fn rejects_token_with_unknown_kid() {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("some-other-key".to_string());
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).unwrap();
        let token = encode(&header, &claims("https://kube.test", "ravn", 600), &key).unwrap();
        assert!(auth().validate(&token).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(auth().validate("not.a.jwt").is_err());
    }
}
