// Vitest unit tests for the UI-only active-Core selection slice (Task 218).
// Pure Zustand state — no Tauri, no DOM.

import { beforeEach, describe, expect, it } from "vitest";

import { useCoresStore } from "./useCoresStore";

beforeEach(() => {
  useCoresStore.setState({ pendingActiveCoreId: null });
});

describe("useCoresStore", () => {
  it("starts with no pending selection", () => {
    expect(useCoresStore.getState().pendingActiveCoreId).toBeNull();
  });

  it("setPendingActiveCore records the selected core_id", () => {
    useCoresStore.getState().setPendingActiveCore("abc123");
    expect(useCoresStore.getState().pendingActiveCoreId).toBe("abc123");
  });

  it("clearPendingActiveCore resets the selection", () => {
    useCoresStore.getState().setPendingActiveCore("abc123");
    useCoresStore.getState().clearPendingActiveCore();
    expect(useCoresStore.getState().pendingActiveCoreId).toBeNull();
  });
});
