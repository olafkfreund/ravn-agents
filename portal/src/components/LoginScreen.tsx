import { useAuth } from "../lib/AuthContext";

/** Shown when the control plane requires user auth and nobody is signed in. */
export function LoginScreen() {
  const { login, error } = useAuth();
  return (
    <div className="grid min-h-dvh place-items-center bg-bg px-4">
      <div className="w-full max-w-sm rounded-2xl border border-line bg-surface p-8 shadow-xl">
        <div className="mb-6 flex items-center gap-3">
          <span className="grid h-10 w-10 place-items-center rounded-xl bg-sev-notice/15 font-display text-xl font-bold text-sev-notice">
            R
          </span>
          <div>
            <h1 className="font-display text-2xl font-bold tracking-tight">Ravn</h1>
            <p className="text-xs text-fg-dim">control plane</p>
          </div>
        </div>

        <p className="mb-6 text-sm text-fg-dim">
          Sign in with your organization account to view the fleet.
        </p>

        <button
          type="button"
          onClick={login}
          className="w-full rounded-lg bg-sev-notice px-4 py-2.5 text-sm font-semibold text-bg transition
                     hover:opacity-90
                     focus:outline-none focus:ring-2 focus:ring-sev-notice/50"
        >
          Sign in with SSO
        </button>

        {error && (
          <p className="mt-4 rounded-lg border border-sev-error/40 bg-sev-error/10 px-3 py-2 text-xs text-sev-error">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
