import type { Config } from "tailwindcss";

// Tailwind v3 — locked here because v4 ships a LightningCSS dep that
// pulls in non-permissive transitive licenses (cargo-deny flags it).
// Phase 2 (Task 24) brings shadcn/ui; the theme stub below is the
// minimum that makes Tailwind compile.
const config: Config = {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {},
  },
  plugins: [],
};

export default config;
