// Portal user authentication (#26): OpenID Connect Authorization Code + PKCE
// against the IdP the control plane is configured with. The access token is
// stored in sessionStorage and presented as a bearer on every API call; the
// control plane validates it and enforces RBAC. No client secret (public SPA).

/** Public auth config from the control plane's `/auth/config`. */
export interface AuthConfig {
  enabled: boolean;
  issuer?: string;
  clientId?: string;
  scopes?: string;
}

const TOKEN_KEY = "ravn_access_token";
const PKCE_KEY = "ravn_pkce";
const CALLBACK_PATH = "/auth/callback";
const redirectUri = () => window.location.origin + CALLBACK_PATH;

export function getToken(): string | null {
  return sessionStorage.getItem(TOKEN_KEY);
}
export function clearToken(): void {
  sessionStorage.removeItem(TOKEN_KEY);
}

/** Fetch the control plane's auth configuration. */
export async function fetchAuthConfig(): Promise<AuthConfig> {
  try {
    const res = await fetch("/auth/config");
    if (!res.ok) return { enabled: false };
    return (await res.json()) as AuthConfig;
  } catch {
    return { enabled: false };
  }
}

interface Discovery {
  authorization_endpoint: string;
  token_endpoint: string;
}

async function discover(issuer: string): Promise<Discovery> {
  const base = issuer.replace(/\/$/, "");
  const res = await fetch(`${base}/.well-known/openid-configuration`);
  if (!res.ok) throw new Error("OIDC discovery failed");
  return (await res.json()) as Discovery;
}

function base64url(bytes: Uint8Array): string {
  let s = "";
  bytes.forEach((b) => (s += String.fromCharCode(b)));
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function randomString(byteLen = 48): string {
  const a = new Uint8Array(byteLen);
  crypto.getRandomValues(a);
  return base64url(a);
}

async function challengeFor(verifier: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return base64url(new Uint8Array(digest));
}

/** Begin the login flow: redirect to the IdP with a PKCE challenge. */
export async function login(cfg: AuthConfig): Promise<void> {
  if (!cfg.issuer || !cfg.clientId) throw new Error("OIDC not configured");
  const { authorization_endpoint, token_endpoint } = await discover(cfg.issuer);
  const verifier = randomString();
  const state = randomString(16);
  sessionStorage.setItem(
    PKCE_KEY,
    JSON.stringify({ verifier, state, token_endpoint, clientId: cfg.clientId }),
  );
  const params = new URLSearchParams({
    response_type: "code",
    client_id: cfg.clientId,
    redirect_uri: redirectUri(),
    scope: cfg.scopes || "openid profile email",
    state,
    code_challenge: await challengeFor(verifier),
    code_challenge_method: "S256",
  });
  window.location.assign(`${authorization_endpoint}?${params.toString()}`);
}

/** Are we currently on the OIDC redirect callback? */
export function isCallback(): boolean {
  return window.location.pathname === CALLBACK_PATH && window.location.search.includes("code=");
}

/** Exchange the authorization code for an access token (PKCE). On success the
 *  token is stored and the URL is cleaned back to the app root. */
export async function handleCallback(): Promise<void> {
  const url = new URL(window.location.href);
  const code = url.searchParams.get("code");
  const state = url.searchParams.get("state");
  const stored = JSON.parse(sessionStorage.getItem(PKCE_KEY) || "{}");
  sessionStorage.removeItem(PKCE_KEY);
  if (!code || !stored.verifier || stored.state !== state) {
    throw new Error("OIDC callback state mismatch");
  }
  const res = await fetch(stored.token_endpoint, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      code,
      redirect_uri: redirectUri(),
      client_id: stored.clientId,
      code_verifier: stored.verifier,
    }),
  });
  if (!res.ok) throw new Error("token exchange failed");
  const data = (await res.json()) as { access_token?: string };
  if (!data.access_token) throw new Error("no access_token in token response");
  sessionStorage.setItem(TOKEN_KEY, data.access_token);
  window.history.replaceState({}, "", "/");
}

/** Discard the token and reload to a clean state. */
export function logout(): void {
  clearToken();
  window.location.assign("/");
}

/** Decode a JWT payload (no verification — display only; the server verifies). */
export function decodeJwt(token: string): Record<string, unknown> | null {
  try {
    const payload = token.split(".")[1];
    return JSON.parse(atob(payload.replace(/-/g, "+").replace(/_/g, "/")));
  } catch {
    return null;
  }
}
