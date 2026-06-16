// Cross-device handoff state tests (Task 518, Tier-2; design/16 §3.12). The
// state ROUND-TRIP: serialize -> token -> restore -> equal state, with version +
// corruption + freshness guards. The real cross-device TRANSPORT is Tier-3.
import {
  HandoffParseError,
  type HandoffState,
  isHandoffFresh,
  restoreHandoff,
  serializeHandoff,
} from "./handoff";

const FULL: HandoffState = {
  coreId: "core-a",
  route: "workspace/[id]",
  params: { id: "ws_123" },
  sessionId: "sess_9",
  sinceOffset: "42",
  capturedAtMs: 1_700_000_000_000,
};

const MINIMAL: HandoffState = {
  coreId: "core-b",
  route: "(tabs)/inbox",
  capturedAtMs: 1_700_000_000_000,
};

describe("handoff round-trip", () => {
  it("serializes and restores a full state exactly", () => {
    const token = serializeHandoff(FULL);
    expect(typeof token).toBe("string");
    // URL/QR-safe: no +, /, or = padding.
    expect(token).not.toMatch(/[+/=]/);
    expect(restoreHandoff(token)).toEqual(FULL);
  });

  it("serializes and restores a minimal state (no optionals)", () => {
    const token = serializeHandoff(MINIMAL);
    const restored = restoreHandoff(token);
    expect(restored).toEqual(MINIMAL);
    expect(restored.params).toBeUndefined();
    expect(restored.sessionId).toBeUndefined();
    expect(restored.sinceOffset).toBeUndefined();
  });

  it("tolerates surrounding whitespace in the token", () => {
    const token = serializeHandoff(MINIMAL);
    expect(restoreHandoff(`  ${token}\n`)).toEqual(MINIMAL);
  });
});

describe("handoff guards", () => {
  it("throws on a non-base64url / garbage token", () => {
    expect(() => restoreHandoff("!!!not a token!!!")).toThrow(HandoffParseError);
  });

  it("throws on a wrong-version envelope", () => {
    // base64url of {"v":99,"s":{...}}
    const bad = Buffer.from(JSON.stringify({ v: 99, s: MINIMAL }))
      .toString("base64")
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/, "");
    expect(() => restoreHandoff(bad)).toThrow(/unsupported handoff version/);
  });

  it("throws when required fields are missing", () => {
    const bad = Buffer.from(JSON.stringify({ v: 1, s: { route: "x" } }))
      .toString("base64")
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/, "");
    expect(() => restoreHandoff(bad)).toThrow(/missing required fields/);
  });
});

describe("isHandoffFresh", () => {
  it("is fresh within the window and stale beyond it", () => {
    const now = FULL.capturedAtMs + 60_000; // 1 min later
    expect(isHandoffFresh(FULL, { now: () => now })).toBe(true);
    const later = FULL.capturedAtMs + 10 * 60_000; // 10 min later
    expect(isHandoffFresh(FULL, { now: () => later })).toBe(false);
  });

  it("rejects a future-dated capture (clock skew guard)", () => {
    const before = FULL.capturedAtMs - 1000;
    expect(isHandoffFresh(FULL, { now: () => before })).toBe(false);
  });
});
