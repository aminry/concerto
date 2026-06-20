//! E2E mock harness install (Task 520). Loaded ONLY when the app URL carries
//! `?mock=1` (see `main.tsx`) — so it is a separate, lazily-imported chunk that
//! never ships in the normal app. It builds a Core-free [`DataClient`] from
//! `@concerto/client/testing`, installs it on `window.__CONCERTO_TEST_DATA_CLIENT__`
//! (the seam `lib/data.ts#makeDataClient` reads), and exposes `window.__mock`
//! so the Playwright spec can push a live `notification.events` frame and prove
//! the new item appears WITHOUT a manual refresh.

import {
  DEVICE_CERT_METADATA_KEY,
  decodeCertHeader,
  encodeCertHeader,
  type EphemeralCertClaims,
} from "@concerto/client";
import {
  createMockDataClient,
  type MockDataClientHandle,
  mockNotificationEvent,
} from "@concerto/client/testing";

import { sessionManager } from "./session";

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
    /** Session driver for the Task 522 Playwright spec (test-only). */
    __session?: {
      /**
       * The `concerto-device-cert` header the live session would attach — proves
       * the minted cert is header-ready in a REAL browser (decodes the kind).
       */
      headerForCurrentSession: () => {
        key: string;
        hasHeader: boolean;
        deviceKind: string | null;
      };
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

  window.__session = {
    headerForCurrentSession() {
      const cert = sessionManager.getCert();
      if (!cert) return { key: DEVICE_CERT_METADATA_KEY, hasHeader: false, deviceKind: null };
      // Encode → decode round-trips the exact header the interceptor attaches.
      const decoded = decodeCertHeader(encodeCertHeader(cert));
      const claims = JSON.parse(decoded.claimsJson) as EphemeralCertClaims;
      return { key: DEVICE_CERT_METADATA_KEY, hasHeader: true, deviceKind: claims.deviceKind };
    },
  };
}
