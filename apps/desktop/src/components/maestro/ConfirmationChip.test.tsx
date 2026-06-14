// @vitest-environment jsdom
//
// Component tests for the write-tool confirmation chip (Task 415). Proves: the
// `AwaitingApproval` frame renders (tool + summary); `urgent` drives the red
// styling + the `destructive_label`; Approve resolves via
// `Sessions.ResolveApproval` (the existing path, reused — no new RPC, no
// bypass) with `ApprovalDecision.APPROVE`; Deny sends `DENY`. The `invoke`
// double stands in for the live shell (414).

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { ConfirmationChip } from "./ConfirmationChip";
import type { AwaitingApproval } from "../../api/sessions";

const approval: AwaitingApproval = {
  approval_id: "ap-9",
  tool: "set_workarea_paused",
  summary: "Pause workarea bach",
  payload_json: "{}",
  urgent: true,
  destructive_label: "Pauses an active agent",
};

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(null);
});

describe("ConfirmationChip", () => {
  it("renders the tool, summary, urgent styling + destructive_label", () => {
    render(<ConfirmationChip sessionId="s1" approval={approval} />);
    expect(screen.getByText("set_workarea_paused")).toBeTruthy();
    expect(screen.getByText("Pause workarea bach")).toBeTruthy();
    expect(screen.getByTestId("destructive-label").textContent).toBe(
      "Pauses an active agent",
    );
    expect(
      screen.getByTestId("confirmation-chip").getAttribute("data-urgent"),
    ).toBe("true");
  });

  it("Approve resolves via Sessions.ResolveApproval with APPROVE (no new RPC)", async () => {
    const onResolved = vi.fn();
    render(
      <ConfirmationChip
        sessionId="s1"
        approval={approval}
        onResolved={onResolved}
      />,
    );
    await userEvent.click(screen.getByText("Approve"));
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Sessions.ResolveApproval",
      payload: { session_id: "s1", approval_id: "ap-9", decision: 1 },
    });
    expect(onResolved).toHaveBeenCalledTimes(1);
  });

  it("Deny resolves with DENY", async () => {
    render(<ConfirmationChip sessionId="s1" approval={approval} />);
    await userEvent.click(screen.getByText("Deny"));
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Sessions.ResolveApproval",
      payload: { session_id: "s1", approval_id: "ap-9", decision: 3 },
    });
  });

  it("renders non-urgent styling when not urgent", () => {
    render(
      <ConfirmationChip
        sessionId="s1"
        approval={{ ...approval, urgent: false }}
      />,
    );
    expect(
      screen.getByTestId("confirmation-chip").getAttribute("data-urgent"),
    ).toBe("false");
    expect(screen.queryByTestId("destructive-label")).toBeNull();
  });
});
