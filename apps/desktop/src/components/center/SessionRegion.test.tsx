// @vitest-environment jsdom
//
// Component tests for the multi-agent session tab strip (Task 323,
// design/15 §3.4). Proves: the "+ new session" menu offers
// claude/codex/gemini (the user-creatable `agent_kind` subset) and
// `createSession` carries the picked kind; an N-session `ListSessions` mock
// renders N tabs; a mocked per-workarea edit-mutex contention frame renders
// the inline "blocked on …" notice without tearing down the strip. The
// mutex itself is server-side (Task 308); this only surfaces its effect.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Capture per-subject event callbacks so a test can drive a live frame, and
// stub subscribe/unsubscribe so the jsdom run has no Tauri runtime. Several
// components (the tab strip + each SessionTab) subscribe to the same
// `session.events.<sid>` subject, so store a list per subject and fan a
// driven frame out to all of them.
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

// The terminal mounts xterm + WebGL; the composer is irrelevant here.
// Stub both so the test only exercises the tab strip + notices.
vi.mock("../SessionTerminal", () => ({
  SessionTerminal: () => <div data-testid="terminal" />,
}));
vi.mock("../SessionComposer", () => ({
  SessionComposer: () => <div data-testid="composer" />,
}));

import { SessionRegion } from "./SessionRegion";
import { renderWithClient } from "../test-utils";
import { useUiStore } from "../../state/useUiStore";

function session(id: string, agent_kind: string, status = "running") {
  return {
    id,
    workarea_id: "wa-1",
    chat_id: `chat-${id}`,
    agent_kind,
    status,
    started_at: [1717459200, 0] as [number, number],
  };
}

function mockSessions(list: ReturnType<typeof session>[]): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    switch (args.method) {
      case "Sessions.ListSessions":
        return Promise.resolve({ sessions: list });
      case "Sessions.CreateSession":
        return Promise.resolve(session("sess-new", "codex", "starting"));
      default:
        return Promise.resolve(undefined);
    }
  });
}

beforeEach(() => {
  invoke.mockReset();
  eventCallbacks.clear();
  useUiStore.setState({ activeSessionId: null });
});

describe("SessionRegion — multi-agent tabs", () => {
  it("offers claude/codex/gemini in the new-session menu (no echo)", async () => {
    mockSessions([session("s1", "claude")]);
    renderWithClient(<SessionRegion workareaId="wa-1" />);

    // Open the end-of-strip "+" menu.
    await userEvent.click(
      await screen.findByRole("button", { name: /new session/i }),
    );

    expect(screen.getByRole("menuitem", { name: /claude/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /codex/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /gemini/i })).toBeInTheDocument();
    expect(
      screen.queryByRole("menuitem", { name: /echo/i }),
    ).not.toBeInTheDocument();
  });

  it("createSession carries the picked agent_kind", async () => {
    mockSessions([session("s1", "claude")]);
    renderWithClient(<SessionRegion workareaId="wa-1" />);

    await userEvent.click(
      await screen.findByRole("button", { name: /new session/i }),
    );
    await userEvent.click(screen.getByRole("menuitem", { name: /gemini/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Sessions.CreateSession",
        payload: expect.objectContaining({
          workarea_id: "wa-1",
          agent_kind: "gemini",
        }),
      }),
    );
  });

  it("renders N tabs for an N-session workarea", async () => {
    mockSessions([
      session("s1", "claude"),
      session("s2", "codex"),
      session("s3", "gemini"),
    ]);
    renderWithClient(<SessionRegion workareaId="wa-1" />);

    expect(await screen.findByText("claude")).toBeInTheDocument();
    expect(screen.getByText("codex")).toBeInTheDocument();
    expect(screen.getByText("gemini")).toBeInTheDocument();
    // Each tab has a close button — three concurrent sessions, three tabs.
    expect(screen.getAllByRole("button", { name: /close session/i })).toHaveLength(
      3,
    );
  });

  it("surfaces the edit-mutex contention effect as a dismissible inline notice", async () => {
    mockSessions([session("s1", "claude")]);
    renderWithClient(<SessionRegion workareaId="wa-1" />);

    // Wait for auto-select + the active session's event subscription.
    await screen.findByText("claude");
    await waitFor(() =>
      expect(eventCallbacks.get("session.events.s1")?.length).toBeGreaterThan(0),
    );

    // Drive a blocked write frame the Core (Task 308) emits on
    // `session.events.<sid>`: an ApprovalResolved whose `decision` carries
    // the typed wire-code + holder description.
    act(() =>
      fireEvent("session.events.s1", {
        offset: 1,
        body: {
          Session: {
            session_id: "s1",
            kind: {
              ApprovalResolved: {
                approval_id: "ap-1",
                tool: "Write",
                decision: "workarea.edit_mutex.blocked: blocked on session s2",
              },
            },
          },
        },
      }),
    );

    const notice = await screen.findByRole("status");
    expect(notice).toHaveTextContent(/blocked on session s2/i);
    // The strip survives — the tab is still there.
    expect(screen.getByText("claude")).toBeInTheDocument();

    // Dismissible.
    await userEvent.click(screen.getByRole("button", { name: /dismiss/i }));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
