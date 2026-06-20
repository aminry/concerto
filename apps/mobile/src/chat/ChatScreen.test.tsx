// ChatScreen tests (Task 512, RN-TL v13.3.3): renders history from a fixture;
// sending appends the user message and streams an assistant reply (mock); the
// unpaired empty state shows the Pair CTA; load error shows retry; a failed send
// marks the user bubble with an inline retry.
import { fireEvent, render, screen, waitFor } from "@testing-library/react-native";

import { ChatScreen } from "./ChatScreen";
import { mockChatClient } from "./chat-client";
import { demoChatFixture, makeTurn } from "./chat-fixtures";

describe("ChatScreen", () => {
  it("renders messages from a fixture", async () => {
    const client = mockChatClient(demoChatFixture());
    render(<ChatScreen client={client} hasCore />);

    expect(await screen.findByTestId("chat-list")).toBeOnTheScreen();
    expect(screen.getByText("What changed on the web workspace today?")).toBeOnTheScreen();
    expect(
      screen.getByText(/Aria opened PR #482/),
    ).toBeOnTheScreen();
  });

  it("sending appends the user message and streams an assistant reply", async () => {
    const client = mockChatClient({
      turns: [makeTurn({ role: "assistant", text: "Hi! How can I help?" })],
      script: (text) => ({ reply: `Echo: ${text}` }),
    });
    render(<ChatScreen client={client} hasCore />);

    await screen.findByTestId("chat-list");

    fireEvent.changeText(screen.getByTestId("composer-input"), "what's up");
    fireEvent.press(screen.getByTestId("composer-send"));

    // The user message appears immediately (optimistic append).
    expect(await screen.findByText("what's up")).toBeOnTheScreen();

    // The streamed assistant reply lands token-by-token.
    await waitFor(() => expect(screen.getByText("Echo: what's up")).toBeOnTheScreen());
  });

  it("does not send blank/whitespace input", async () => {
    const client = mockChatClient({ script: { reply: "nope" } });
    const sendSpy = jest.spyOn(client, "send");
    render(<ChatScreen client={client} hasCore />);
    await screen.findByTestId("chat-empty");

    fireEvent.changeText(screen.getByTestId("composer-input"), "   ");
    fireEvent.press(screen.getByTestId("composer-send"));
    expect(sendSpy).not.toHaveBeenCalled();
  });

  it("shows the unpaired empty state with a Pair CTA when no Core is paired", async () => {
    const onPair = jest.fn();
    const client = mockChatClient(demoChatFixture());
    render(<ChatScreen client={client} hasCore={false} onPair={onPair} />);

    expect(await screen.findByTestId("chat-empty-unpaired")).toBeOnTheScreen();
    // History is NOT loaded while unpaired (no transcript shown).
    expect(screen.queryByTestId("chat-list")).toBeNull();

    fireEvent.press(screen.getByTestId("pair-entry-pair"));
    expect(onPair).toHaveBeenCalledTimes(1);
  });

  it("shows the empty conversation state when there is no history", async () => {
    const client = mockChatClient({ turns: [], script: { reply: "x" } });
    render(<ChatScreen client={client} hasCore />);
    expect(await screen.findByTestId("chat-empty")).toBeOnTheScreen();
  });

  it("shows the load error state with a retry control", async () => {
    const client = mockChatClient(demoChatFixture(), { historyFailWith: "core unreachable" });
    render(<ChatScreen client={client} hasCore />);

    expect(await screen.findByTestId("chat-error")).toBeOnTheScreen();
    expect(screen.getByText("core unreachable")).toBeOnTheScreen();
    const retry = screen.getByTestId("chat-retry");
    expect(retry).toBeOnTheScreen();
    fireEvent.press(retry);
    // Same client still fails, so the error persists (the retry re-issues the call).
    expect(await screen.findByTestId("chat-error")).toBeOnTheScreen();
  });

  it("marks a failed send with an inline retry and recovers on retry", async () => {
    // First mount: history ok, but send fails. Then we swap send to succeed and
    // press the inline retry to prove the recovery path re-issues the send.
    const client = mockChatClient(
      { turns: [], script: (text) => ({ reply: `ok: ${text}` }) },
      { sendFailWith: "offline" },
    );
    render(<ChatScreen client={client} hasCore />);
    await screen.findByTestId("chat-empty");

    fireEvent.changeText(screen.getByTestId("composer-input"), "hello");
    fireEvent.press(screen.getByTestId("composer-send"));

    // The user bubble shows the inline "Failed · Retry".
    expect(await screen.findByText("Failed · Retry")).toBeOnTheScreen();

    // Now make send succeed and tap the inline retry.
    jest.spyOn(client, "send").mockImplementation(async (text: string) => {
      async function* tokens() {
        yield `ok: ${text}`;
      }
      return { tokens: tokens() };
    });
    fireEvent.press(screen.getByText("Failed · Retry"));

    await waitFor(() => expect(screen.getByText("ok: hello")).toBeOnTheScreen());
  });
});
