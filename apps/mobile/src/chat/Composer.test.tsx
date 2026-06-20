// Composer tests (Task 512 + Task 515, RN-TL v13.3.3): send button behaviour and
// voice dictation — mic start shows the live partial, final fills the composer,
// and the permission-denied path surfaces an error without listening.
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react-native";

import { Composer } from "./Composer";
import { createMockRecognizer } from "../voice/speech-recognizer";

describe("Composer", () => {
  it("sends trimmed text and clears the input", async () => {
    const onSend = jest.fn();
    render(<Composer onSend={onSend} />);
    fireEvent.changeText(screen.getByTestId("composer-input"), "  hi there  ");
    fireEvent.press(screen.getByTestId("composer-send"));
    expect(onSend).toHaveBeenCalledWith("hi there");
    expect(screen.getByTestId("composer-input").props.value).toBe("");
  });

  it("hides the mic when no recognizer is available", () => {
    render(<Composer onSend={jest.fn()} recognizer={createMockRecognizer({}, { available: false })} />);
    expect(screen.queryByTestId("composer-mic")).toBeNull();
  });

  it("dictation: start shows the live partial, final fills the composer", async () => {
    const rec = createMockRecognizer({
      partials: ["book", "book a meeting"],
      final: "book a meeting",
    });
    render(<Composer onSend={jest.fn()} recognizer={rec} />);

    await act(async () => {
      fireEvent.press(screen.getByTestId("composer-mic"));
    });

    // Listening: the live partial preview shows the latest partial.
    expect(screen.getByTestId("composer-partial")).toBeOnTheScreen();
    expect(screen.getByText("book a meeting")).toBeOnTheScreen();

    // Stop -> final fills the composer input.
    await act(async () => {
      fireEvent.press(screen.getByTestId("composer-mic"));
    });
    await waitFor(() =>
      expect(screen.getByTestId("composer-input").props.value).toBe("book a meeting"),
    );
    expect(screen.queryByTestId("composer-partial")).toBeNull();
  });

  it("dictation appends the final onto text already typed", async () => {
    const rec = createMockRecognizer({ final: "and then ship it" }, { autoFinal: true });
    render(<Composer onSend={jest.fn()} recognizer={rec} />);
    fireEvent.changeText(screen.getByTestId("composer-input"), "review the PR");

    await act(async () => {
      fireEvent.press(screen.getByTestId("composer-mic"));
    });
    await waitFor(() =>
      expect(screen.getByTestId("composer-input").props.value).toBe(
        "review the PR and then ship it",
      ),
    );
  });

  it("permission-denied: shows an error and does not start listening", async () => {
    const rec = createMockRecognizer(
      { partials: ["should not appear"] },
      { permission: "denied" },
    );
    render(<Composer onSend={jest.fn()} recognizer={rec} />);

    await act(async () => {
      fireEvent.press(screen.getByTestId("composer-mic"));
    });

    expect(screen.getByTestId("composer-mic-error")).toBeOnTheScreen();
    expect(screen.getByText(/Microphone access denied/)).toBeOnTheScreen();
    expect(screen.queryByTestId("composer-partial")).toBeNull();
    expect(screen.getByTestId("composer-input").props.value).toBe("");
  });
});
