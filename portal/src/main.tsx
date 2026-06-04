import { StrictMode, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import { App } from "./App";
import { AuthProvider, useAuth } from "./lib/AuthContext";
import { LoginScreen } from "./components/LoginScreen";
import "./index.css";

const queryClient = new QueryClient();

/** Gate the app on auth state: a brief splash while resolving, the login
 *  screen when auth is required and nobody is signed in, otherwise the app. */
function AuthGate({ children }: { children: ReactNode }) {
  const { status, required } = useAuth();
  if (status === "loading") {
    return (
      <div className="grid min-h-dvh place-items-center bg-bg text-fg-dim">
        <span className="animate-pulse font-display text-lg">Ravn…</span>
      </div>
    );
  }
  if (required && status === "anonymous") return <LoginScreen />;
  return <>{children}</>;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <AuthGate>
          <BrowserRouter>
            <App />
          </BrowserRouter>
        </AuthGate>
      </AuthProvider>
    </QueryClientProvider>
  </StrictMode>,
);
