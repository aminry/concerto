import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Concerto Web Client (Task 519). A React SPA over the Core's connect-web
// bridge (gRPC-Web). Port 5174 to avoid clashing with the desktop dev server
// (5173).
export default defineConfig({
  plugins: [react()],
  server: { port: 5174 },
  build: { outDir: "dist", target: "es2022" },
});
