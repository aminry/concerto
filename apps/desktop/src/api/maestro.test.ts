// Vitest unit tests for the Maestro binding (Task 415). Mocks
// `@tauri-apps/api/core`'s `invoke` so the `Maestro.*` request shapes + the
// `maestro.events` opaque-frame decode are pinned without a running Tauri
// shell (mirrors `cores.test.ts`). The live `Maestro.*` dispatch arm is 414's;
// here every call answers against the mocked double.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  decodeMaestroEvent,
  getDigest,
  MAESTRO_EVENTS_SUBJECT,
  MaestroVisibility,
  sendToMaestro,
  setWorkareaVisibility,
  type Digest,
} from "./maestro";

beforeEach(() => {
  invoke.mockReset();
});

describe("maestro bindings", () => {
  it("sendToMaestro forwards Maestro.SendToMaestro with text + empty attachments", async () => {
    invoke.mockResolvedValueOnce(null);
    await sendToMaestro("@bach rebase onto main");
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Maestro.SendToMaestro",
      payload: { text: "@bach rebase onto main", attachments: [] },
    });
  });

  it("getDigest calls Maestro.GetDigest with an empty request and returns the Digest", async () => {
    const digest: Digest = {
      text: "Finished: 2 PRs merged.",
      chips: [
        {
          rule_id: "r1",
          workarea_id: "wa1",
          title: "Review bach",
          priority: 3,
          created_at_ms: 1717459200000,
          action: "open_workarea",
        },
      ],
      generated_at_ms: 1717459200000,
      stale: false,
    };
    invoke.mockResolvedValueOnce(digest);
    const out = await getDigest();
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Maestro.GetDigest",
      payload: {},
    });
    // The frozen field names round-trip verbatim (snake_case on the wire).
    expect(out.text).toBe("Finished: 2 PRs merged.");
    expect(out.chips[0].rule_id).toBe("r1");
    expect(out.chips[0].created_at_ms).toBe(1717459200000);
    expect(out.generated_at_ms).toBe(1717459200000);
  });

  it("setWorkareaVisibility forwards the snake_case workarea_id + enum tag", async () => {
    invoke.mockResolvedValueOnce(null);
    await setWorkareaVisibility("wa-7", MaestroVisibility.HARD_FACTS_ONLY);
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Maestro.SetWorkareaVisibility",
      payload: { workarea_id: "wa-7", visibility: 2 },
    });
  });
});

describe("maestro.events decode", () => {
  it("exposes the unscoped subject string", () => {
    expect(MAESTRO_EVENTS_SUBJECT).toBe("maestro.events");
  });

  it("decodes a snake_case message frame", () => {
    const ev = decodeMaestroEvent({ message: { text: "hi", role: "maestro" } });
    expect(ev).toEqual({ kind: "message", text: "hi", role: "maestro" });
  });

  it("decodes a PascalCase (prost serde default) message frame", () => {
    const ev = decodeMaestroEvent({ Message: { text: "hi" } });
    expect(ev.kind).toBe("message");
    if (ev.kind === "message") expect(ev.text).toBe("hi");
  });

  it("decodes routing_executed into a typed target list", () => {
    const ev = decodeMaestroEvent({
      routing_executed: { targets: ["bach", "mozart"], summary: "queued" },
    });
    expect(ev).toEqual({
      kind: "routing_executed",
      targets: ["bach", "mozart"],
      summary: "queued",
    });
  });

  it("decodes digest_generated with a nested digest", () => {
    const ev = decodeMaestroEvent({
      digest_generated: { digest: { text: "fresh", chips: [] } },
    });
    expect(ev.kind).toBe("digest_generated");
    if (ev.kind === "digest_generated")
      expect(ev.digest?.text).toBe("fresh");
  });

  it("decodes budget_exhausted + disabled_by_policy", () => {
    expect(decodeMaestroEvent({ budget_exhausted: {} }).kind).toBe(
      "budget_exhausted",
    );
    const policy = decodeMaestroEvent({
      disabled_by_policy: { reason: "enterprise_data_privacy" },
    });
    expect(policy).toEqual({
      kind: "disabled_by_policy",
      reason: "enterprise_data_privacy",
    });
  });

  it("decodes an opaque checks_opaque byte frame (the 414 carrier shape)", () => {
    const inner = JSON.stringify({ message: { text: "from bytes" } });
    const bytes = Array.from(new TextEncoder().encode(inner));
    const ev = decodeMaestroEvent({ checks_opaque: bytes });
    expect(ev.kind).toBe("message");
    if (ev.kind === "message") expect(ev.text).toBe("from bytes");
  });

  it("degrades an unrecognized frame to { kind: unknown } without throwing", () => {
    const ev = decodeMaestroEvent({ some_future_event: { x: 1 } });
    expect(ev.kind).toBe("unknown");
    expect(decodeMaestroEvent(null).kind).toBe("unknown");
  });
});
