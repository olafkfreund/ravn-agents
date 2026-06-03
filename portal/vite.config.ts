import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Backend to proxy API calls to in dev. Override with VITE_API_PROXY; point it
// at whatever RAVN_BIND the control plane uses (default port is common, so set
// an uncommon one locally).
const apiTarget = process.env.VITE_API_PROXY || "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5318,
    proxy: {
      "/api": { target: apiTarget, changeOrigin: true },
      "/health": { target: apiTarget, changeOrigin: true },
      "/ready": { target: apiTarget, changeOrigin: true },
      "/openapi.json": { target: apiTarget, changeOrigin: true },
    },
  },
});
