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
use serde_json::Value;

/// API access role (#26). Safe methods need a viewer; mutating methods need
/// admin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Admin,
}

/// Verify a JWT's RS256 signature against `jwks`, plus issuer (and audience
/// when given), returning its claims. `alg` is pinned to RS256 rather than
/// trusting the header (avoids algorithm-confusion attacks). Shared by the
/// agent ServiceAccount path (#57) and the user OIDC path (#26).
fn verify(jwks: &JwkSet, issuer: &str, audience: Option<&str>, token: &str) -> anyhow::Result<Value> {
    let header = decode_header(token).context("decoding token header")?;
    let kid = header.kid.context("token has no `kid`")?;
    let jwk = jwks
        .find(&kid)
        .ok_or_else(|| anyhow!("no JWKS key matches token `kid` {kid}"))?;
    let key = DecodingKey::from_jwk(jwk).context("building decoding key from JWK")?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer]);
    match audience {
        Some(aud) => validation.set_audience(&[aud]),
        None => validation.validate_aud = false,
    }

    let data = decode::<Value>(token, &key, &validation).context("verifying token signature/claims")?;
    Ok(data.claims)
}

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
        let claims = verify(&self.jwks, &self.issuer, Some(&self.audience), token)?;
        let sub = claims
            .get("sub")
            .and_then(Value::as_str)
            .context("token has no `sub`")?
            .to_string();
        Ok(SaClaims { sub })
    }
}

/// Kubernetes `TokenReview` fallback for ingest auth (#102): validate a
/// presented ServiceAccount token by asking the cluster API, instead of
/// verifying a JWKS signature locally. Useful when the cluster's OIDC JWKS is
/// not reachable from the (external) control plane. Requires the control plane
/// to hold a credential with `system:auth-delegator`-style access.
pub struct TokenReviewValidator {
    http: reqwest::Client,
    url: String,
    bearer: String,
    audience: String,
}

#[derive(serde::Deserialize)]
struct TokenReviewResponse {
    #[serde(default)]
    status: TokenReviewStatus,
}

#[derive(serde::Deserialize, Default)]
struct TokenReviewStatus {
    #[serde(default)]
    authenticated: bool,
    #[serde(default)]
    user: ReviewUser,
    #[serde(default)]
    error: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct ReviewUser {
    #[serde(default)]
    username: String,
}

impl TokenReviewValidator {
    /// Build a validator. `api_url` is the cluster API base; the TokenReview
    /// path is appended. `ca_pem` (optional) is the API server's CA bundle.
    pub fn new(
        api_url: &str,
        bearer: String,
        ca_pem: Option<&str>,
        audience: String,
    ) -> anyhow::Result<Self> {
        let mut builder = reqwest::Client::builder();
        if let Some(pem) = ca_pem {
            builder = builder
                .add_root_certificate(reqwest::Certificate::from_pem(pem.as_bytes()).context("parsing TokenReview CA")?);
        }
        let http = builder.build().context("building TokenReview HTTP client")?;
        let url = format!(
            "{}/apis/authentication.k8s.io/v1/tokenreviews",
            api_url.trim_end_matches('/')
        );
        Ok(Self { http, url, bearer, audience })
    }

    /// Submit a TokenReview for `token` and return its identity if the cluster
    /// reports it authenticated for our audience.
    pub async fn validate(&self, token: &str) -> anyhow::Result<SaClaims> {
        let body = serde_json::json!({
            "apiVersion": "authentication.k8s.io/v1",
            "kind": "TokenReview",
            "spec": { "token": token, "audiences": [self.audience] },
        });
        let resp = self
            .http
            .post(&self.url)
            .bearer_auth(&self.bearer)
            .json(&body)
            .send()
            .await
            .context("calling TokenReview API")?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("TokenReview API returned {status}");
        }
        let review: TokenReviewResponse = resp.json().await.context("parsing TokenReview response")?;
        if !review.status.authenticated {
            anyhow::bail!(
                "token not authenticated{}",
                review.status.error.map(|e| format!(": {e}")).unwrap_or_default()
            );
        }
        Ok(SaClaims { sub: review.status.user.username })
    }
}

/// Validates portal **user** OIDC tokens and maps a claim to an API [`Role`]
/// (#26). Stateless: the SPA obtains an OIDC token from the IdP and presents it
/// as a bearer; the control plane verifies it against the IdP's JWKS and
/// derives the role from the user's groups.
pub struct UserAuth {
    issuer: String,
    /// Expected audience (the OIDC client id). `None` skips the aud check.
    audience: Option<String>,
    jwks: JwkSet,
    /// Claim holding the user's group memberships (default `groups`).
    groups_claim: String,
    /// Group whose members get [`Role::Admin`].
    admin_group: Option<String>,
    /// When set, only members of this group get [`Role::Viewer`]; otherwise any
    /// authenticated user is a viewer.
    viewer_group: Option<String>,
}

impl UserAuth {
    pub fn new(
        issuer: String,
        audience: Option<String>,
        jwks_json: &str,
        groups_claim: String,
        admin_group: Option<String>,
        viewer_group: Option<String>,
    ) -> anyhow::Result<Self> {
        let jwks: JwkSet = serde_json::from_str(jwks_json).context("parsing OIDC JWKS")?;
        if jwks.keys.is_empty() {
            return Err(anyhow!("OIDC JWKS contains no keys"));
        }
        Ok(Self { issuer, audience, jwks, groups_claim, admin_group, viewer_group })
    }

    /// Verify a user's token and resolve their role. `Err` means the token is
    /// invalid; `Ok(None)` means a valid user who is not authorized for any
    /// role (authenticated but not in an allowed group).
    pub fn authorize(&self, token: &str) -> anyhow::Result<Option<Role>> {
        let claims = verify(&self.jwks, &self.issuer, self.audience.as_deref(), token)?;
        let groups = extract_groups(&claims, &self.groups_claim);

        if let Some(admin) = &self.admin_group {
            if groups.iter().any(|g| g == admin) {
                return Ok(Some(Role::Admin));
            }
        }
        let viewer_ok = match &self.viewer_group {
            Some(vg) => groups.iter().any(|g| g == vg),
            None => true, // any authenticated user is at least a viewer
        };
        Ok(viewer_ok.then_some(Role::Viewer))
    }
}

/// Pull group memberships from a claim that may be an array of strings or a
/// single space/comma-separated string.
fn extract_groups(claims: &Value, claim: &str) -> Vec<String> {
    match claims.get(claim) {
        Some(Value::Array(items)) => {
            items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        }
        Some(Value::String(s)) => {
            s.split([',', ' ']).filter(|t| !t.is_empty()).map(str::to_string).collect()
        }
        _ => Vec::new(),
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

    // --- user OIDC / RBAC (#26) ---

    fn sign_value(claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("ravn-test-key".to_string());
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).unwrap();
        encode(&header, claims, &key).unwrap()
    }

    fn user_token(groups: serde_json::Value) -> String {
        let exp = (chrono::Utc::now().timestamp() + 600) as usize;
        sign_value(&serde_json::json!({
            "sub": "alice",
            "iss": "https://idp.test",
            "aud": "ravn-portal",
            "exp": exp,
            "groups": groups,
        }))
    }

    fn user_auth(admin: Option<&str>, viewer: Option<&str>) -> UserAuth {
        UserAuth::new(
            "https://idp.test".into(),
            Some("ravn-portal".into()),
            &test_jwks(),
            "groups".into(),
            admin.map(str::to_string),
            viewer.map(str::to_string),
        )
        .unwrap()
    }

    #[test]
    fn admin_group_member_is_admin() {
        let token = user_token(serde_json::json!(["staff", "ravn-admins"]));
        let role = user_auth(Some("ravn-admins"), None).authorize(&token).unwrap();
        assert_eq!(role, Some(Role::Admin));
    }

    #[test]
    fn non_admin_authenticated_user_is_viewer_by_default() {
        let token = user_token(serde_json::json!(["staff"]));
        let role = user_auth(Some("ravn-admins"), None).authorize(&token).unwrap();
        assert_eq!(role, Some(Role::Viewer));
    }

    #[test]
    fn viewer_group_gates_access() {
        let token = user_token(serde_json::json!(["staff"]));
        // viewer_group required, user is not a member -> authenticated but no role.
        let role = user_auth(Some("ravn-admins"), Some("ravn-viewers")).authorize(&token).unwrap();
        assert_eq!(role, None);
    }

    #[test]
    fn user_token_for_another_audience_is_rejected() {
        // Audience confusion guard (#100): a token minted for a different
        // relying party at the same issuer must NOT authenticate here.
        let exp = (chrono::Utc::now().timestamp() + 600) as usize;
        let token = sign_value(&serde_json::json!({
            "sub": "mallory", "iss": "https://idp.test", "aud": "some-other-app",
            "exp": exp, "groups": ["ravn-admins"],
        }));
        assert!(user_auth(Some("ravn-admins"), None).authorize(&token).is_err());
    }

    #[test]
    fn space_separated_groups_claim_is_parsed() {
        let token = user_token(serde_json::json!("staff ravn-admins"));
        let role = user_auth(Some("ravn-admins"), None).authorize(&token).unwrap();
        assert_eq!(role, Some(Role::Admin));
    }

    #[test]
    fn token_review_response_parses_authenticated() {
        let json = r#"{"status":{"authenticated":true,"user":{"username":"system:serviceaccount:ravn:ravn-controller","groups":["g"]}}}"#;
        let r: TokenReviewResponse = serde_json::from_str(json).unwrap();
        assert!(r.status.authenticated);
        assert_eq!(r.status.user.username, "system:serviceaccount:ravn:ravn-controller");
    }

    #[test]
    fn token_review_response_parses_unauthenticated() {
        let json = r#"{"status":{"authenticated":false,"error":"invalid bearer token"}}"#;
        let r: TokenReviewResponse = serde_json::from_str(json).unwrap();
        assert!(!r.status.authenticated);
        assert_eq!(r.status.error.as_deref(), Some("invalid bearer token"));
    }

    #[test]
    fn user_token_with_bad_signature_is_rejected() {
        // Wrong issuer fails verification regardless of groups.
        let exp = (chrono::Utc::now().timestamp() + 600) as usize;
        let token = sign_value(&serde_json::json!({
            "sub": "mallory", "iss": "https://evil.test", "aud": "ravn-portal",
            "exp": exp, "groups": ["ravn-admins"],
        }));
        assert!(user_auth(Some("ravn-admins"), None).authorize(&token).is_err());
    }
}
