import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 2 dev workflow: Vite serves the renderer on 5173; the Tauri Rust
// shell points its dev URL at the same port. The `clearScreen: false`
// flag lets `pnpm tauri dev` interleave Vite and Cargo logs cleanly.
//
// Task 47 — Monaco diff editor:
//
// The `monaco-editor` package ships ESM entry points for its workers
// (`monaco-editor/esm/vs/editor/editor.worker?worker`, etc.). They are
// imported lazily inside `DiffViewer.tsx` via a `MonacoEnvironment`
// `getWorker` hook, so Vite tree-shakes the unused language workers
// out of the main bundle. No extra Vite plugin is required — the
// default `optimizeDeps` config below just keeps `monaco-editor` out
// of the dependency pre-bundle (the worker imports are otherwise
// duplicated, which throws at runtime).
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
  optimizeDeps: {
    // Monaco's ESM worker entry points are resolved at runtime via the
    // `MonacoEnvironment` hook; pre-bundling them confuses Vite into
    // shipping two copies. Excluding the package keeps the dev server
    // and `vite build` paths consistent.
    exclude: ["monaco-editor"],
  },
});
