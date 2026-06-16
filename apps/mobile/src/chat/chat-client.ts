// Mobile Concerto-chat data seam (Task 512). The Concerto chat landing tab reads
// through this narrow, transport-agnostic interface so the RN component tree
// stays decoupled from the live Maestro transport — exactly mirroring how
// `src/data/workspaces-client.ts` (Task 513) wraps a mock for Tier-2.
//
// User-facing name is "Concerto" (D14); the internal service is **Maestro**. We
// reuse the REAL generated `Maestro` proto types from @concerto/client (mobile
// consumes ONLY @concerto/client, PHASE5_PLANNING D11) so the screen is exercised
// against the same contract the live transport will satisfy:
//   - history : `Maestro.GetHistory` (unary) -> MaestroHistory { turns: MaestroTurn[] }
//                where MaestroTurn { role: "user"|"assistant", text, createdAtMs }.
//   - send    : `Maestro.SendToMaestro` (unary, MaestroMessageRequest { text }).
//   - stream  : assistant tokens arrive live on the `maestro.events` Streams
//                subject (the same one Desktop appends after seeding GetHistory).
//
// The LIVE implementation over a `DataClient` (createClient(Maestro, dc.transport)
// + dc.subscribe("maestro.events", …)) is wired by a later task — decoding the
// `maestro.events` Event body is a Core-side wire detail (Tier-3). Until then the
// screen runs against `mockChatClient(...)`, which streams a scripted reply token
// by token from an in-memory fixture.
import type { MaestroTurn } from "@concerto/client/gen/concerto/v1/maestro_pb";

/** A chat turn as the screen renders it. Re-exports the generated `MaestroTurn`. */
export type ChatTurn = MaestroTurn;

/** A live assistant-token stream handle returned by [`ChatClient.send`]. */
export interface AssistantStream {
  /**
   * Async iterable of assistant token chunks (text deltas). The screen appends
   * each chunk to the in-flight assistant bubble; the iterator completing marks
   * the turn done. Throwing rejects the turn (the screen shows send-failed +
   * retry).
   */
  tokens: AsyncIterable<string>;
}

/**
 * The screen-facing data contract for the Concerto chat. Every method is a
 * Promise / async-iterable so the live implementation can issue real unary RPCs
 * + ride the `maestro.events` server stream; the mock resolves from fixtures.
 */
export interface ChatClient {
  /** Load the persisted chat history, oldest-first (`Maestro.GetHistory`). */
  history(): Promise<ChatTurn[]>;
  /**
   * Send the user's text (`Maestro.SendToMaestro`) and return a handle whose
   * `tokens` stream the assistant's reply live. The caller has already optimistically
   * appended the user turn; this resolves once the send is accepted (so a reject
   * here is a send failure), then streams the assistant reply.
   */
  send(text: string): Promise<AssistantStream>;
}

/** A scripted assistant reply for [`mockChatClient`]: a full text, chunked on send. */
export interface MockChatScript {
  /** The assistant's full reply text (streamed token-by-token). */
  reply: string;
  /** Tokenizer for the reply — defaults to whitespace-preserving word chunks. */
  chunk?: (reply: string) => string[];
}

/** In-memory fixture backing [`mockChatClient`]. */
export interface ChatFixture {
  /** Seed history turns, oldest-first. */
  turns?: ChatTurn[];
  /**
   * The scripted reply for the NEXT `send`. A function lets a test vary the reply
   * per call (e.g. echo the prompt); a static script replies the same each time.
   */
  script?: MockChatScript | ((text: string) => MockChatScript);
}

/** Options for [`mockChatClient`]. */
export interface MockChatOptions {
  /**
   * If set, `history()` rejects with this error — drives the screen's load-error
   * + retry state. The string is surfaced verbatim in the UI.
   */
  historyFailWith?: string;
  /**
   * If set, `send()` rejects with this error — drives the per-message send-failed
   * + retry affordance on the user bubble.
   */
  sendFailWith?: string;
  /**
   * Artificial delay (ms) before `history()` resolves — lets a test observe the
   * loading state deterministically. Defaults to 0 (resolve on next microtask).
   */
  historyDelayMs?: number;
  /**
   * Per-token delay (ms) for the streamed reply. Defaults to 0 (tokens arrive on
   * back-to-back microtasks) so tests can `await` the stream without timers.
   */
  tokenDelayMs?: number;
}

/** Default tokenizer: keep words AND their trailing whitespace so the join is lossless. */
export function defaultChunk(reply: string): string[] {
  return reply.match(/\S+\s*/g) ?? (reply ? [reply] : []);
}

function delay(ms: number | undefined): Promise<void> {
  if (!ms || ms <= 0) return Promise.resolve();
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Build a fixture-backed [`ChatClient`] for tests + the pre-live-transport app
 * shell. `send` streams the scripted reply token-by-token through the
 * [`AssistantStream`]. Returns the REAL generated `MaestroTurn` types so the
 * screen is exercised against the contract the live transport will satisfy.
 */
export function mockChatClient(
  fixture: ChatFixture = {},
  opts: MockChatOptions = {},
): ChatClient {
  const turns = fixture.turns ?? [];
  return {
    async history() {
      await delay(opts.historyDelayMs);
      if (opts.historyFailWith) throw new Error(opts.historyFailWith);
      return turns;
    },
    async send(text: string) {
      if (opts.sendFailWith) throw new Error(opts.sendFailWith);
      const script =
        typeof fixture.script === "function"
          ? fixture.script(text)
          : (fixture.script ?? { reply: "" });
      const chunks = (script.chunk ?? defaultChunk)(script.reply);
      const tokenDelayMs = opts.tokenDelayMs;
      async function* tokens(): AsyncGenerator<string> {
        for (const c of chunks) {
          await delay(tokenDelayMs);
          yield c;
        }
      }
      return { tokens: tokens() };
    },
  };
}
