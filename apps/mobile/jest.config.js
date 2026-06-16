// Mobile unit-test harness (Task 508, decision D13): jest + the `jest-expo`
// preset + @testing-library/react-native. This is the `pnpm -C apps/mobile test`
// gate wired into the mobile CI lane (.github/workflows/mobile.yml).
//
// `transformIgnorePatterns` whitelists the RN/Expo/expo-router/@concerto ESM
// packages so Babel transpiles them (node_modules ship untranspiled ESM that
// jest's CJS runtime can't load otherwise). The trailing `@concerto` entry lets
// the workspace-linked `@concerto/client` source be transformed too.
//
// pnpm note: deps live under `node_modules/.pnpm/<name>@<ver>/node_modules/<name>`,
// so the leading segment must allow the optional `.pnpm/...@.../node_modules/`
// prefix before the package name — otherwise the default jest-expo pattern (which
// assumes a hoisted layout) never matches and untranspiled RN ESM reaches the
// CJS runtime ("Cannot use import statement outside a module").
/** @type {import('jest').Config} */
module.exports = {
  preset: "jest-expo",
  setupFilesAfterEnv: ["<rootDir>/jest.setup.ts"],
  transformIgnorePatterns: [
    "node_modules/(?!(?:.pnpm/[^/]+/node_modules/)?((jest-)?react-native|@react-native(-community)?|expo(nent)?|@expo(nent)?/.*|@expo-google-fonts/.*|react-navigation|@react-navigation/.*|@unimodules/.*|unimodules|sentry-expo|native-base|react-native-svg|@bufbuild/.*|@connectrpc/.*|@concerto/.*))",
  ],
};
