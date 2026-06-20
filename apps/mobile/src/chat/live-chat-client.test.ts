// createLiveChatClient tests (Task 512, Tier-2). Proves the LIVE chat client's
// unary paths round-trip the REAL `Maestro` proto types through the OPAQUE-BYTES
// native module (the same identity-codec mock the native-data-client adapter
// uses): `getHistory` decodes a `MaestroHistory`, `sendToMaestro` delivers the
// text. The assistant token STREAM is Tier-3 (see live-chat-client.ts header), so
// `send`'s returned stream is currently empty — asserted here so the contract is
// pinned.
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";

import {
  MaestroHistorySchema,
  MaestroMessageRequestSchema,
} from "@concerto/client/gen/concerto/v1/maestro_pb";

import { createLiveChatClient } from "./live-chat-client";
import { createNativeDataClient } from "../data/native-data-client";
import { createMockConcertoIroh } from "../native/mock-concerto-iroh";

const GET_HISTORY = "/concerto.v1.Maestro/GetHistory";
const SEND = "/concerto.v1.Maestro/SendToMaestro";

async function openClient(handlers: Parameters<typeof createMockConcertoIroh>[0]) {
  const module = createMockConcertoIroh(handlers);
  const handle = await module.openSession(
    { endpointId: "ep", directAddrs: [], coreNoisePub: "00" },
    new Uint8Array([1]),
  );
  return createLiveChatClient(createNativeDataClient(module, handle));
}

describe("createLiveChatClient", () => {
  it("history() decodes a real MaestroHistory over the native transport", async () => {
    const client = await openClient({
      unary: {
        [GET_HISTORY]: () =>
          toBinary(
            MaestroHistorySchema,
            create(MaestroHistorySchema, {
              turns: [
                { role: "user", text: "hi", createdAtMs: 1700000000000n },
                { role: "assistant", text: "hello", createdAtMs: 1700000001000n },
              ],
            }),
          ),
      },
    });

    const turns = await client.history();
    expect(turns).toHaveLength(2);
    expect(turns[0].role).toBe("user");
    expect(turns[1].text).toBe("hello");
    expect(turns[0].createdAtMs).toBe(1700000000000n);
  });

  it("send() delivers the text via SendToMaestro (Tier-3 token stream is empty)", async () => {
    let seenText: string | undefined;
    const client = await openClient({
      unary: {
        [SEND]: (payload) => {
          seenText = fromBinary(MaestroMessageRequestSchema, payload).text;
          return new Uint8Array(0); // Empty response
        },
      },
    });

    const stream = await client.send("schedule a review");
    expect(seenText).toBe("schedule a review");

    const chunks: string[] = [];
    for await (const c of stream.tokens) chunks.push(c);
    expect(chunks).toEqual([]); // Tier-3: live token decode deferred
  });
});
