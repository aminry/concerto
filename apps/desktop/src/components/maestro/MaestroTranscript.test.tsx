// Unit tests for the transcript line mappers (Task 8 / 415). Pins `historyToLines`
// (the persisted-history seed used on reload) and the history+events ordering
// contract: history lines render first (oldest-first), live events append after.

import { describe, expect, it } from "vitest";

import { eventsToLines, historyToLines } from "./MaestroTranscript";
import type { MaestroEvent, MaestroTurn } from "../../api/maestro";

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
