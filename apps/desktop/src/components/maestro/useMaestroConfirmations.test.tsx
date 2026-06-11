// @vitest-environment jsdom
//
// Tests for the Maestro write-tool confirmation-chip PRODUCER (Task 417,
// design/08 R-2). Proves: `readAwaitingApproval` lifts a write-tool
// `AwaitingApproval` off a `session.events.<sid>` frame (both prost-serde
// PascalCase + snake_case spellings, mirroring `SessionRegion`); the hook
// subscribes to `Maestro.GetState.maestro_session_id`'s stream, drops a
// hand-built frame into `pendingConfirmation`, `<ConfirmationChip>` renders it,
// and Approve calls `Sessions.ResolveApproval` (the existing path — no bypass);
// and an empty session id opens no subscription.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Capture per-subject event callbacks so a test can drive a live frame, and
// stub subscribe/unsubscribe so the jsdom run has no Tauri runtime (the
// SessionRegion.test pattern).
const eventCallbacks = new Map<string, Array<(payload: unknown) => void>>();
function fireEvent(subject: string, payload: unknown): void {
  for (const cb of eventCallbacks.get(subject) ?? []) cb(payload);
}
vi.mock("../../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/client")>();
  return {
    ...actual,
    onConcertoEvent: vi.fn(
      async (subject: string, cb: (payload: unknown) => void) => {
        const list = eventCallbacks.get(subject) ?? [];
        list.push(cb);
        eventCallbacks.set(subject, list);
        return () => {
          const cur = eventCallbacks.get(subject);
          if (cur) eventCallbacks.set(subject, cur.filter((c) => c !== cb));
        };
      },
    ),
    subscribe: vi.fn(async () => "sub-1"),
    unsubscribe: vi.fn(async () => {}),
  };
});

import {
  readAwaitingApproval,
  useMaestroConfirmations,
} from "./useMaestroConfirmations";
import { ConfirmationChip } from "./ConfirmationChip";
import { useMaestroStore } from "../../state/useMaestroStore";

const SID = "sess-maestro-1";

/// Build a `session.events.<sid>` frame carrying an `AwaitingApproval` under
/// the given oneof spelling.
function approvalFrame(
  outer: "Session" | "session",
  inner: "AwaitingApproval" | "awaiting_approval",
) {
  return {
    offset: 1,
    body: {
      [outer]: {
        session_id: SID,
        kind: {
          [inner]: {
            approval_id: "ap-1",
            tool: "create_workspace",
            summary: "Create workspace “payments”",
            payload_json: "{}",
            urgent: true,
            destructive_label: "create-workspace",
          },
        },
      },
    },
  };
}

/// A tiny harness: mount the producer pointed at `sessionId`, render the chip
/// when a pending confirmation lands.
function Harness({ sessionId }: { sessionId: string }): JSX.Element {
  useMaestroConfirmations(sessionId);
  const pending = useMaestroStore((s) => s.pendingConfirmation);
  const setPending = useMaestroStore((s) => s.setPendingConfirmation);
  if (!pending) return <div data-testid="no-chip" />;
  return (
    <ConfirmationChip
      sessionId={pending.sessionId}
      approval={pending.approval}
      onResolved={() => setPending(null)}
    />
  );
}

beforeEach(() => {
  invoke.mockReset();
  eventCallbacks.clear();
  useMaestroStore.setState({ pendingConfirmation: null });
});

afterEach(() => {
  useMaestroStore.setState({ pendingConfirmation: null });
});

describe("readAwaitingApproval", () => {
  it("reads a PascalCase Session/AwaitingApproval frame", () => {
    const a = readAwaitingApproval(approvalFrame("Session", "AwaitingApproval"));
    expect(a?.approval_id).toBe("ap-1");
    expect(a?.tool).toBe("create_workspace");
    expect(a?.urgent).toBe(true);
  });

  it("reads a snake_case session/awaiting_approval frame", () => {
    const a = readAwaitingApproval(
      approvalFrame("session", "awaiting_approval"),
    );
    expect(a?.approval_id).toBe("ap-1");
  });

  it("returns null for a non-approval frame", () => {
    expect(
      readAwaitingApproval({
        offset: 2,
        body: { Session: { session_id: SID, kind: { Exited: {} } } },
      }),
    ).toBeNull();
    expect(readAwaitingApproval(null)).toBeNull();
    expect(readAwaitingApproval({ body: {} })).toBeNull();
  });
});

describe("useMaestroConfirmations producer", () => {
  it("lifts an AwaitingApproval frame into the chip and Approve calls Sessions.ResolveApproval", async () => {
    invoke.mockResolvedValue(null);
    render(<Harness sessionId={SID} />);

    // The producer subscribed to the Maestro session's events.
    await waitFor(() =>
      expect(
        eventCallbacks.get(`session.events.${SID}`)?.length,
      ).toBeGreaterThan(0),
    );

    // Drive a write-tool approval frame.
    act(() =>
      fireEvent(
        `session.events.${SID}`,
        approvalFrame("Session", "AwaitingApproval"),
      ),
    );

    // The chip renders (urgent → destructive label shows).
    const chip = await screen.findByTestId("confirmation-chip");
    expect(chip.getAttribute("data-urgent")).toBe("true");
    expect(screen.getByTestId("destructive-label")).toHaveTextContent(
      "create-workspace",
    );

    // Approve resolves via the existing ResolveApproval path (no bypass, R-2).
    await userEvent.click(screen.getByRole("button", { name: /approve/i }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Sessions.ResolveApproval",
        payload: { session_id: SID, approval_id: "ap-1", decision: 1 },
      }),
    );
    // Cleared on resolve.
    await waitFor(() =>
      expect(useMaestroStore.getState().pendingConfirmation).toBeNull(),
    );
  });

  it("opens no subscription when the Maestro session id is empty (disabled)", () => {
    render(<Harness sessionId="" />);
    expect(eventCallbacks.size).toBe(0);
    expect(screen.getByTestId("no-chip")).toBeInTheDocument();
  });
});
