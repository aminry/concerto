// Type-only augmentation so `tsc` (typecheck/lint) knows about the
// `@testing-library/jest-dom` matchers the Task 219 component tests use
// (`toBeInTheDocument`, `toHaveTextContent`, `toBeDisabled`, …). The runtime
// registration happens in `vitest.setup.ts`; importing the `/vitest` subpath
// here wires the matcher types onto vitest's `Assertion` so the `*.test.tsx`
// assertions type-check. Lives under `src/` so the tsconfig `include: ["src"]`
// picks it up.
import "@testing-library/jest-dom/vitest";
