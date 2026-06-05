import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Backend to proxy API calls to in dev. Defaults to the docker-compose control
// plane (host port 18090 — see docker-compose.yml). Override with VITE_API_PROXY
// to point at a directly-run server (e.g. http://127.0.0.1:8080 for `ravn-server`
// with its default RAVN_BIND).
const apiTarget = process.env.VITE_API_PROXY || "http://127.0.0.1:18090";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5318,
    proxy: {
      "/api": { target: apiTarget, changeOrigin: true },
      "/health": { target: apiTarget, changeOrigin: true },
      "/ready": { target: apiTarget, changeOrigin: true },
      "/openapi.json": { target: apiTarget, changeOrigin: true },
      "/ws": { target: apiTarget, changeOrigin: true, ws: true },
    },
  },
});
