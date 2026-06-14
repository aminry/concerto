// Unit tests for the transcript line mappers (Task 8 / 415). Pins `historyToLines`
// (the persisted-history seed used on reload) and the history+events ordering
// contract: history lines render first (oldest-first), live events append after.

import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";

import {
  eventsToLines,
  historyToLines,
  isNearBottom,
  MaestroTranscript,
  waitingAfterEvent,
} from "./MaestroTranscript";
import type { MaestroEvent, MaestroTurn } from "../../api/maestro";

describe("waitingAfterEvent (working-indicator transitions)", () => {
  it("turns ON when the user's turn is forwarded to the model", () => {
    expect(
      waitingAfterEvent({ kind: "message", text: "hi", role: "user" }),
    ).toBe(true);
  });

  it("turns OFF when the assistant reply arrives", () => {
    expect(
      waitingAfterEvent({ kind: "message", text: "hi back", role: "assistant" }),
    ).toBe(false);
  });

  it("turns OFF on routing dispatch and budget/policy stops (no reply coming)", () => {
    expect(
      waitingAfterEvent({ kind: "routing_executed", targets: ["bach"] }),
    ).toBe(false);
    expect(waitingAfterEvent({ kind: "budget_exhausted" })).toBe(false);
    expect(
      waitingAfterEvent({ kind: "disabled_by_policy", reason: "x" }),
    ).toBe(false);
  });

  it("leaves the state unchanged (null) for non-conversational events", () => {
    expect(waitingAfterEvent({ kind: "digest_generated" })).toBeNull();
    expect(waitingAfterEvent({ kind: "unknown", raw: {} })).toBeNull();
  });
});

describe("MaestroTranscript working indicator", () => {
  it("renders the typing indicator when busy, even with no messages yet", () => {
    render(<MaestroTranscript lines={[]} busy />);
    expect(screen.getByTestId("maestro-typing")).toBeTruthy();
    // The empty-state copy is replaced by the indicator while busy.
    expect(screen.queryByTestId("transcript-empty")).toBeNull();
  });

  it("does not render the indicator when idle", () => {
    render(
      <MaestroTranscript
        lines={[{ id: "m-0", kind: "message", text: "hi", role: "user" }]}
      />,
    );
    expect(screen.queryByTestId("maestro-typing")).toBeNull();
  });
});

describe("isNearBottom (auto-scroll pin decision)", () => {
  it("is true when scrolled exactly to the bottom", () => {
    expect(
      isNearBottom({ scrollTop: 400, scrollHeight: 600, clientHeight: 200 }),
    ).toBe(true);
  });

  it("is true within the threshold of the bottom", () => {
    // 8px from the bottom (< 24px default threshold).
    expect(
      isNearBottom({ scrollTop: 392, scrollHeight: 600, clientHeight: 200 }),
    ).toBe(true);
  });

  it("is false when the user has scrolled up past the threshold", () => {
    // 200px from the bottom — the user is reading scrollback.
    expect(
      isNearBottom({ scrollTop: 200, scrollHeight: 600, clientHeight: 200 }),
    ).toBe(false);
  });

  it("is true when content is shorter than the viewport (nothing to scroll)", () => {
    expect(
      isNearBottom({ scrollTop: 0, scrollHeight: 120, clientHeight: 200 }),
    ).toBe(true);
  });

  it("honors a custom threshold", () => {
    const m = { scrollTop: 350, scrollHeight: 600, clientHeight: 200 }; // 50px up
    expect(isNearBottom(m, 24)).toBe(false);
    expect(isNearBottom(m, 80)).toBe(true);
  });
});

describe("historyToLines", () => {
  it("maps persisted turns to role-tagged message lines, oldest-first", () => {
    const turns: MaestroTurn[] = [
      { role: "user", text: "hi", created_at_ms: 10 },
      { role: "assistant", text: "hi back", created_at_ms: 30 },
    ];
    const lines = historyToLines(turns);
    expect(lines).toHaveLength(2);
    expect(lines[0]).toMatchObject({ kind: "message", text: "hi", role: "user" });
    expect(lines[1]).toMatchObject({
      kind: "message",
      text: "hi back",
      role: "assistant",
    });
    // History ids are `hist-`-prefixed so they never collide with live ids.
    expect(lines[0].id).toBe("hist-0");
  });

  it("returns an empty list for no history", () => {
    expect(historyToLines([])).toEqual([]);
  });

  it("history seeds the top; live events append after (reload contract)", () => {
    const history: MaestroTurn[] = [{ role: "user", text: "earlier", created_at_ms: 1 }];
    const events: MaestroEvent[] = [
      { kind: "message", text: "live reply", role: "assistant" },
    ];
    const combined = [...historyToLines(history), ...eventsToLines(events)];
    expect(combined.map((l) => l.text)).toEqual(["earlier", "live reply"]);
  });
});
