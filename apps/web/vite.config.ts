import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Concerto Web Client (Task 519). A React SPA over the Core's connect-web
// bridge (gRPC-Web). Port 5174 to avoid clashing with the desktop dev server
// (5173).
export default defineConfig({
  plugins: [react()],
  // Bind IPv4 explicitly so the Playwright harness (and curl) reach it at
  // 127.0.0.1 — `localhost` resolves to ::1 on some machines.
  server: { host: "127.0.0.1", port: 5174, strictPort: true },
  preview: { host: "127.0.0.1", port: 4173, strictPort: true },
  build: { outDir: "dist", target: "es2022" },
});
