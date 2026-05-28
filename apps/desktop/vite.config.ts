import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 2 dev workflow: Vite serves the renderer on 5173; the Tauri Rust
// shell points its dev URL at the same port. The `clearScreen: false`
// flag lets `pnpm tauri dev` interleave Vite and Cargo logs cleanly.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // Target Chromium 105 / Safari 15 — the WebView floors that Tauri 2
    // supports on macOS / Windows / Linux per design/00 §6.8.
    target: ["es2022", "chrome105", "safari15"],
  },
});
