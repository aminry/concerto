// Shared vitest setup. Loaded for every test file (see `vitest.config.ts`).
//
// `@testing-library/jest-dom` registers the DOM assertion matchers
// (`toBeInTheDocument`, `toBeDisabled`, …) used by the Task 219 component tests.
// It only augments `expect`; it is inert for the node-env data-layer tests.
//
// React Testing Library's automatic per-test cleanup is enabled by importing
// `@testing-library/react`'s afterEach hook indirectly — RTL registers it when
// `globals` is true (the default with this config), so no manual `afterEach` is
// needed here.
import "@testing-library/jest-dom/vitest";
