// Deterministic Concerto-chat fixtures (Task 512) built from @concerto/client's
// REAL generated `MaestroTurn` schema via `create(...)`. Used by the app shell
// (pre-live-transport) and the unit tests so the chat tree renders a stable,
// type-checked transcript without a live Core (PHASE5_PLANNING D11).
import { create, type MessageInitShape } from "@bufbuild/protobuf";

import { MaestroTurnSchema, type MaestroTurn } from "@concerto/client/gen/concerto/v1/maestro_pb";

import type { ChatFixture } from "./chat-client";

type TurnInit = MessageInitShape<typeof MaestroTurnSchema>;

const MINUTE = 60_000;

function agoMs(ms: number): bigint {
  return BigInt(Date.now() - ms);
}

/** Build a strict `MaestroTurn` from loose field overrides. */
export function makeTurn(over: TurnInit & { role: string; text: string }): MaestroTurn {
  return create(MaestroTurnSchema, {
    createdAtMs: agoMs(5 * MINUTE),
    ...over,
  });
}

/**
 * A representative seed transcript + a scripted assistant reply for the app
 * shell and the default in tests: a short prior exchange, then `send` streams a
 * canned multi-token reply that echoes the user's prompt so the demo feels live.
 */
export function demoChatFixture(): ChatFixture {
  return {
    turns: [
      makeTurn({
        role: "user",
        text: "What changed on the web workspace today?",
        createdAtMs: agoMs(8 * MINUTE),
      }),
      makeTurn({
        role: "assistant",
        text: "Aria opened PR #482 (landing hero + nav) and it's awaiting review. Two sessions ran — one finished, one is active.",
        createdAtMs: agoMs(7 * MINUTE),
      }),
    ],
    script: (text: string) => ({
      reply: `Got it — “${text.trim()}”. I'll route that to the right workspace and follow up here as agents make progress.`,
    }),
  };
}
