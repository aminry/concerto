// Metro config for the pnpm monorepo (Task 508). Expo's default config assumes a
// single project root; in a pnpm workspace the shared `@concerto/client` package
// lives at the repo root and deps are symlinked, so Metro must (1) watch the
// workspace root and (2) resolve modules from both the app and the root
// node_modules. This is the Expo-documented monorepo setup.
const { getDefaultConfig } = require("expo/metro-config");
const path = require("path");

const projectRoot = __dirname;
const workspaceRoot = path.resolve(projectRoot, "../..");

const config = getDefaultConfig(projectRoot);

// 1. Watch the whole monorepo so changes to packages/* trigger reloads.
config.watchFolders = [workspaceRoot];

// 2. Resolve modules from the app first, then the hoisted workspace root.
config.resolver.nodeModulesPaths = [
  path.resolve(projectRoot, "node_modules"),
  path.resolve(workspaceRoot, "node_modules"),
];

// 3. pnpm uses symlinks; let Metro follow them.
config.resolver.unstable_enableSymlinks = true;

module.exports = config;
