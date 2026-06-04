import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { getMe, type Role } from "./api";
import {
  decodeJwt,
  fetchAuthConfig,
  getToken,
  handleCallback,
  isCallback,
  login as oidcLogin,
  logout as oidcLogout,
  type AuthConfig,
} from "./auth";

type Status = "loading" | "anonymous" | "authenticated";

interface AuthState {
  status: Status;
  /** True when the control plane requires user auth. */
  required: boolean;
  /** Subject (username/email) of the signed-in user, if any. */
  user: string | null;
  /** Resolved API role; `admin` when auth is disabled. */
  role: Role;
  isAdmin: boolean;
  login: () => void;
  logout: () => void;
  error: string | null;
}

const AuthCtx = createContext<AuthState | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status>("loading");
  const [config, setConfig] = useState<AuthConfig>({ enabled: false });
  const [user, setUser] = useState<string | null>(null);
  const [role, setRole] = useState<Role>("admin");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const cfg = await fetchAuthConfig();
        if (cancelled) return;
        setConfig(cfg);

        // Auth disabled: open access, admin role.
        if (!cfg.enabled) {
          setRole("admin");
          setStatus("authenticated");
          return;
        }

        // Returning from the IdP: finish the code exchange.
        if (isCallback()) {
          await handleCallback();
        }

        const token = getToken();
        if (!token) {
          if (!cancelled) setStatus("anonymous");
          return;
        }

        const claims = decodeJwt(token);
        setUser(
          (claims?.email as string) ||
            (claims?.preferred_username as string) ||
            (claims?.sub as string) ||
            "user",
        );
        setRole(await getMe());
        if (!cancelled) setStatus("authenticated");
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : "authentication failed");
          setStatus("anonymous");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const value: AuthState = {
    status,
    required: config.enabled,
    user,
    role,
    isAdmin: role === "admin",
    login: () => {
      oidcLogin(config).catch((e) =>
        setError(e instanceof Error ? e.message : "login failed"),
      );
    },
    logout: oidcLogout,
    error,
  };

  return <AuthCtx.Provider value={value}>{children}</AuthCtx.Provider>;
}

export function useAuth(): AuthState {
  const ctx = useContext(AuthCtx);
  if (!ctx) throw new Error("useAuth must be used within AuthProvider");
  return ctx;
}
