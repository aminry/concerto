import { defineConfig } from "vitest/config";

// Unit tests (vitest) live under `src/**/*.test.ts`. The Playwright UI-E2E specs
// under `e2e/**/*.spec.ts` are driven by `pnpm e2e`, NOT vitest — scope the
// include so `vitest run` never tries to collect a Playwright `test()` call.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
