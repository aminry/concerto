// The LIVE Concerto-chat client over a `DataClient` (Task 512 seam → Tier-3 wire).
//
// This is the real-transport implementation of the `ChatClient` seam: it speaks
// the generated `Maestro` service over the native `DataClient` (the same seam the
// Workspaces/Inbox screens use) —
//   - history : createClient(Maestro, dc.transport).getHistory({}) → MaestroHistory,
//   - send    : createClient(Maestro, dc.transport).sendToMaestro({ text }),
//   - stream  : dc.subscribe("maestro.events", …) for the assistant token deltas.
//
// TIER-3: the assistant token stream depends on decoding the `maestro.events`
// `Event` body (a Core-side oneof whose renderable-token shape is owned by the
// Maestro service, not the mobile client). Wiring + verifying that against a live
// Core needs a real device build + a running Core, so the token-extraction step
// is deferred and clearly marked. The unary history/send paths are exercised by
// the native-data-client adapter tests (Task 510); the chat SCREEN is fully
// covered Tier-2 via `mockChatClient`. We export this so the app-client factory
// can swap it in once the live token contract is verified, without touching the
// screen.
import { createClient } from "@connectrpc/connect";

import type { DataClient } from "@concerto/client";
import { Maestro } from "@concerto/client/gen/concerto/v1/maestro_pb";

import type { AssistantStream, ChatClient, ChatTurn } from "./chat-client";

/** The Streams subject carrying live assistant token deltas (mirrors Desktop). */
export const MAESTRO_EVENTS_SUBJECT = "maestro.events";

/**
 * Build a live [`ChatClient`] over a [`DataClient`]. The unary paths are live;
 * the assistant token stream is a TIER-3 seam (see file header) — until the
 * `maestro.events` Event-body token contract is verified against a live Core,
 * `send` resolves with an empty token stream (the user turn is still delivered
 * via `sendToMaestro`; replies will surface once the stream decoder lands).
 */
export function createLiveChatClient(dc: DataClient): ChatClient {
  const maestro = createClient(Maestro, dc.transport);
  return {
    async history(): Promise<ChatTurn[]> {
      const res = await maestro.getHistory({});
      return res.turns;
    },
    async send(text: string): Promise<AssistantStream> {
      await maestro.sendToMaestro({ text });
      // TIER-3: decode `maestro.events` Event frames into assistant token deltas
      // and bridge `dc.subscribe(MAESTRO_EVENTS_SUBJECT, …)` to this iterable.
      // Verified on a live Core in a later task; empty for now.
      async function* tokens(): AsyncGenerator<string> {
        // intentionally empty until the live token contract is verified (Tier-3)
      }
      return { tokens: tokens() };
    },
  };
}
