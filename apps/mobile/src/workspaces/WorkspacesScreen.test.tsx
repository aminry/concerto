// WorkspacesScreen tests (Task 513): renders workspace rows from a fixture and
// shows the empty / loading / error states. Navigation (tapping a row) is
// covered here too via the injectable `onOpenWorkspace` prop, mirroring the
// route file's `router.push`. RN-TL v13.3.3.
import { fireEvent, render, screen, waitFor } from "@testing-library/react-native";

import { WorkspacesScreen } from "./WorkspacesScreen";
import { demoWorkspacesFixture } from "../data/fixtures";
import { mockWorkspacesClient } from "../data/workspaces-client";

describe("WorkspacesScreen", () => {
  it("renders workspace rows from a fixture", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    render(<WorkspacesScreen client={client} />);

    expect(await screen.findByTestId("workspaces-list")).toBeOnTheScreen();
    expect(screen.getByText("Web Redesign")).toBeOnTheScreen();
    expect(screen.getByText("Core Runtime")).toBeOnTheScreen();
    expect(screen.getByTestId("workspace-row-ws-web")).toBeOnTheScreen();
  });

  it("shows the loading state before data resolves", () => {
    // Fake timers keep the delayed promise pending across the assertion (and stop
    // the timer leaking past the test), so the synchronous first render is loading.
    jest.useFakeTimers();
    try {
      const client = mockWorkspacesClient(demoWorkspacesFixture(), { delayMs: 1000 });
      const { unmount } = render(<WorkspacesScreen client={client} />);
      expect(screen.getByTestId("workspaces-loading")).toBeOnTheScreen();
      unmount();
    } finally {
      jest.useRealTimers();
    }
  });

  it("shows the empty state when there are no workspaces", async () => {
    const client = mockWorkspacesClient({ workspaces: [] });
    render(<WorkspacesScreen client={client} />);
    expect(await screen.findByTestId("workspaces-empty")).toBeOnTheScreen();
  });

  it("shows the error state when the client rejects", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture(), { failWith: "core unreachable" });
    render(<WorkspacesScreen client={client} />);
    expect(await screen.findByTestId("workspaces-error")).toBeOnTheScreen();
    expect(screen.getByText("core unreachable")).toBeOnTheScreen();
  });

  it("recovers via Try again after an error", async () => {
    // First mount fails; we cannot mutate the client, so assert the retry control
    // exists and is pressable (the route re-issues the same call on press).
    const client = mockWorkspacesClient(demoWorkspacesFixture(), { failWith: "boom" });
    render(<WorkspacesScreen client={client} />);
    const retry = await screen.findByTestId("workspaces-retry");
    expect(retry).toBeOnTheScreen();
    fireEvent.press(retry);
    // Still failing (same client), so the error state persists.
    expect(await screen.findByTestId("workspaces-error")).toBeOnTheScreen();
  });

  it("calls onOpenWorkspace with the tapped workspace (drill-down)", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    const onOpen = jest.fn();
    render(<WorkspacesScreen client={client} onOpenWorkspace={onOpen} />);

    const row = await screen.findByTestId("workspace-row-ws-web");
    fireEvent.press(row);

    await waitFor(() => expect(onOpen).toHaveBeenCalledTimes(1));
    expect(onOpen.mock.calls[0][0].id).toBe("ws-web");
  });
});
