// Unit tests for the UI-only Maestro Zustand slice (Task 415). The store holds
// only UI ephemera (composer draft / collapse flags / pending-confirmation
// selection); the digest/state/transcript are React-Query-canonical and are
// NOT in this slice (design/15 §3.3).

import { beforeEach, describe, expect, it } from "vitest";

import { useMaestroStore } from "./useMaestroStore";
import type { AwaitingApproval } from "../api/sessions";

const approval: AwaitingApproval = {
  approval_id: "ap-1",
  tool: "create_workarea",
  summary: "Create workarea on web",
  payload_json: "{}",
  urgent: true,
  destructive_label: "Creates a worktree",
};

beforeEach(() => {
  useMaestroStore.setState({
    composerDraft: "",
    digestCollapsed: false,
    chatCollapsed: false,
    pendingConfirmation: null,
  });
});

describe("useMaestroStore", () => {
  it("sets the composer draft", () => {
    useMaestroStore.getState().setComposerDraft("@bach hi");
    expect(useMaestroStore.getState().composerDraft).toBe("@bach hi");
  });

  it("toggles the digest + chat collapse flags", () => {
    useMaestroStore.getState().toggleDigestCollapsed();
    expect(useMaestroStore.getState().digestCollapsed).toBe(true);
    useMaestroStore.getState().toggleChatCollapsed();
    expect(useMaestroStore.getState().chatCollapsed).toBe(true);
    useMaestroStore.getState().setDigestCollapsed(false);
    expect(useMaestroStore.getState().digestCollapsed).toBe(false);
  });

  it("holds + clears the pending confirmation selection", () => {
    useMaestroStore
      .getState()
      .setPendingConfirmation({ sessionId: "s1", approval });
    expect(useMaestroStore.getState().pendingConfirmation?.sessionId).toBe("s1");
    expect(
      useMaestroStore.getState().pendingConfirmation?.approval.tool,
    ).toBe("create_workarea");
    useMaestroStore.getState().setPendingConfirmation(null);
    expect(useMaestroStore.getState().pendingConfirmation).toBeNull();
  });
});
