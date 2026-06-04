import { defineConfig } from "vitest/config";

// Task 218: vitest unit tests for the data layer (`src/api/cores.ts`) + the
// active-Core Zustand slice. These mock `@tauri-apps/api`'s `invoke` and touch
// no DOM, so the lightweight `node` environment is sufficient (no jsdom dep).
// Component/DOM tests, if added later, can opt into `jsdom` per-file via the
// `// @vitest-environment jsdom` pragma.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
    globals: false,
  },
});
