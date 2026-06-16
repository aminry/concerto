// mockChatClient tests (Task 512, Tier-2). Proves the fixture-backed seam returns
// the REAL generated `MaestroTurn` history and streams a scripted assistant reply
// token-by-token through the `AssistantStream`, plus the history/send error paths.
import { mockChatClient, defaultChunk } from "./chat-client";
import { demoChatFixture, makeTurn } from "./chat-fixtures";

async function drain(stream: AsyncIterable<string>): Promise<string[]> {
  const out: string[] = [];
  for await (const c of stream) out.push(c);
  return out;
}

describe("mockChatClient", () => {
  it("returns seeded history (real MaestroTurn types), oldest-first", async () => {
    const client = mockChatClient(demoChatFixture());
    const turns = await client.history();
    expect(turns).toHaveLength(2);
    expect(turns[0].role).toBe("user");
    expect(turns[1].role).toBe("assistant");
    expect(typeof turns[0].createdAtMs).toBe("bigint");
  });

  it("streams a scripted reply token-by-token, losslessly", async () => {
    const client = mockChatClient({ script: { reply: "Hello there friend" } });
    const stream = await client.send("hi");
    const chunks = await drain(stream.tokens);
    expect(chunks.length).toBeGreaterThan(1);
    expect(chunks.join("")).toBe("Hello there friend");
  });

  it("lets the script vary the reply per send (echo the prompt)", async () => {
    const client = mockChatClient({ script: (text) => ({ reply: `You said ${text}` }) });
    const stream = await client.send("ping");
    expect((await drain(stream.tokens)).join("")).toBe("You said ping");
  });

  it("rejects history() with the configured error", async () => {
    const client = mockChatClient(demoChatFixture(), { historyFailWith: "core unreachable" });
    await expect(client.history()).rejects.toThrow("core unreachable");
  });

  it("rejects send() with the configured error", async () => {
    const client = mockChatClient({ script: { reply: "x" } }, { sendFailWith: "send failed" });
    await expect(client.send("hi")).rejects.toThrow("send failed");
  });

  it("defaultChunk keeps trailing whitespace so chunks rejoin losslessly", () => {
    const chunks = defaultChunk("two  spaces here");
    expect(chunks.join("")).toBe("two  spaces here");
  });

  it("makeTurn builds a strict MaestroTurn from loose overrides", () => {
    const t = makeTurn({ role: "assistant", text: "hi", createdAtMs: 42n });
    expect(t.role).toBe("assistant");
    expect(t.text).toBe("hi");
    expect(t.createdAtMs).toBe(42n);
  });
});
