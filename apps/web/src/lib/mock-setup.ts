//! E2E mock harness install (Task 520). Loaded ONLY when the app URL carries
//! `?mock=1` (see `main.tsx`) — so it is a separate, lazily-imported chunk that
//! never ships in the normal app. It builds a Core-free [`DataClient`] from
//! `@concerto/client/testing`, installs it on `window.__CONCERTO_TEST_DATA_CLIENT__`
//! (the seam `lib/data.ts#makeDataClient` reads), and exposes `window.__mock`
//! so the Playwright spec can push a live `notification.events` frame and prove
//! the new item appears WITHOUT a manual refresh.

import {
  createMockDataClient,
  type MockDataClientHandle,
  mockNotificationEvent,
} from "@concerto/client/testing";

declare global {
  interface Window {
    /** Driver handle for the Playwright mock spec. */
    __mock?: {
      /** Add a notification to the backing inbox + emit a `created` stream frame. */
      pushLive: (id: string, title: string) => void;
      /** Force the live stream to error so the app drops to the polling fallback. */
      failStream: () => void;
      handle: MockDataClientHandle;
    };
  }
}

/** Install the mock data client + driver. Idempotent. */
export function installMock(): void {
  if (window.__mock) return;
  const handle = createMockDataClient({
    inbox: [
      { id: "seed-1", title: "Agent completed in bach", severity: "low" },
      { id: "seed-2", title: "PR #7 ready to merge", severity: "low" },
    ],
  });
  window.__CONCERTO_TEST_DATA_CLIENT__ = handle.dataClient;

  let offset = 100;
  window.__mock = {
    handle,
    pushLive(id, title) {
      handle.addNotification({ id, title, severity: "high" });
      offset += 1;
      handle.emit(mockNotificationEvent(offset, { kind: "created", id }));
    },
    failStream() {
      handle.failStream();
    },
  };
}
