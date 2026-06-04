import { defineConfig } from "vitest/config";

// Vitest config for the desktop renderer test suite.
//
// Task 218 added the data-layer tests (`src/**/*.test.ts`) which mock
// `@tauri-apps/api`'s `invoke` and touch no DOM — those stay on the lightweight
// `node` environment.
//
// Task 219 adds component/DOM tests for the pairing surfaces
// (`src/**/*.test.tsx`). React Testing Library needs a DOM, so those files run
// under `jsdom`. Rather than flip the whole suite to `jsdom` (which would slow
// the node-only data tests and pull jsdom globals into them), the per-file
// `environmentMatchGlobs` rule below scopes `jsdom` to `*.test.tsx` and keeps
// `*.test.ts` on `node`. The `setupFiles` entry wires
// `@testing-library/jest-dom` matchers + cleanup, but is a no-op for the
// node-env data tests (it only registers matchers when a DOM is present).
export default defineConfig({
  test: {
    environment: "node",
    environmentMatchGlobs: [["src/**/*.test.tsx", "jsdom"]],
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    setupFiles: ["./vitest.setup.ts"],
    globals: true,
  },
});
