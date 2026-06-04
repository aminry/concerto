// Shared test helpers for the Task 219 pairing-UI component tests.
//
// `renderWithClient` wraps a component in a fresh `QueryClientProvider` (retry
// off, no caching across tests) so React-Query-backed components render against
// the mocked `invoke`. Each call gets its own client to avoid cross-test cache
// bleed.
//
// NOTE: not a `*.test.tsx` file, so vitest's `include` glob does not treat it
// as a suite — it has no tests of its own.

import type { ReactElement } from "react";
import { render, type RenderResult } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

export function renderWithClient(ui: ReactElement): RenderResult {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0, staleTime: 0 },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}
