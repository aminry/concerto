// WorkareaDetailScreen tests (Task 513): the segmented Sessions / Code & PRs
// view defaults to Sessions, and tapping "Code & PRs" switches the content. Also
// covers the per-segment loading/empty/error and the workspace->workarea
// resolution (NO project tier — D14). RN-TL v13.3.3.
import { fireEvent, render, screen } from "@testing-library/react-native";

import { WorkareaDetailScreen } from "./WorkareaDetailScreen";
import { demoWorkspacesFixture } from "../data/fixtures";
import { mockWorkspacesClient } from "../data/workspaces-client";

describe("WorkareaDetailScreen", () => {
  it("resolves the workspace's workarea and shows its header", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    render(<WorkareaDetailScreen client={client} workspaceId="ws-web" />);

    expect(await screen.findByTestId("workarea-detail-screen")).toBeOnTheScreen();
    // aria is ws-web's workarea (Workspace -> Workarea).
    expect(await screen.findByText("aria")).toBeOnTheScreen();
    expect(screen.getByText("concerto/aria")).toBeOnTheScreen();
    // Let the default segment settle so no async setState lands post-test.
    await screen.findByTestId("sessions-list");
  });

  it("defaults to the Sessions segment", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    render(<WorkareaDetailScreen client={client} workspaceId="ws-web" />);

    expect(await screen.findByTestId("sessions-list")).toBeOnTheScreen();
    // ws-web/wa-aria has 2 sessions.
    expect(screen.getByTestId("session-se-1")).toBeOnTheScreen();
    expect(screen.getByTestId("session-se-2")).toBeOnTheScreen();
    // The Code & PRs content is not mounted yet.
    expect(screen.queryByTestId("code-list")).toBeNull();
  });

  it("switches to Code & PRs when its segment is tapped", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    render(<WorkareaDetailScreen client={client} workspaceId="ws-web" />);

    // Wait for the default segment to mount, then tap "Code & PRs".
    await screen.findByTestId("sessions-list");
    fireEvent.press(screen.getByTestId("seg-code"));

    expect(await screen.findByTestId("code-list")).toBeOnTheScreen();
    expect(screen.getByTestId("pr-pr-1")).toBeOnTheScreen();
    expect(screen.getByText("Refresh the landing hero + nav")).toBeOnTheScreen();
    // The Sessions content is unmounted once we switch.
    expect(screen.queryByTestId("sessions-list")).toBeNull();
  });

  it("expands a PR's diff inline when its card is tapped (Task 514)", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    render(<WorkareaDetailScreen client={client} workspaceId="ws-web" />);

    await screen.findByTestId("sessions-list");
    fireEvent.press(screen.getByTestId("seg-code"));
    await screen.findByTestId("code-list");

    // The diff is not mounted until the PR card is tapped.
    expect(screen.queryByTestId("pr-diff-view-pr-1")).toBeNull();
    fireEvent.press(screen.getByTestId("pr-toggle-pr-1"));

    // The DiffView mounts and renders the fixture diff's content.
    expect(await screen.findByTestId("pr-diff-view-pr-1")).toBeOnTheScreen();
    expect(screen.getByText("README.md")).toBeOnTheScreen();

    // Tapping again collapses it.
    fireEvent.press(screen.getByTestId("pr-toggle-pr-1"));
    expect(screen.queryByTestId("pr-diff-view-pr-1")).toBeNull();
  });

  it("shows the empty PR state for a workarea with no PRs", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    render(<WorkareaDetailScreen client={client} workspaceId="ws-core" />);

    // ws-core/wa-bee has 1 session and 0 PRs.
    await screen.findByTestId("sessions-list");
    fireEvent.press(screen.getByTestId("seg-code"));
    expect(await screen.findByTestId("code-empty")).toBeOnTheScreen();
  });

  it("shows the loading state before the workarea resolves", () => {
    jest.useFakeTimers();
    try {
      const client = mockWorkspacesClient(demoWorkspacesFixture(), { delayMs: 1000 });
      const { unmount } = render(<WorkareaDetailScreen client={client} workspaceId="ws-web" />);
      expect(screen.getByTestId("workarea-loading")).toBeOnTheScreen();
      unmount();
    } finally {
      jest.useRealTimers();
    }
  });

  it("shows the error state when the client rejects", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture(), { failWith: "core unreachable" });
    render(<WorkareaDetailScreen client={client} workspaceId="ws-web" />);
    expect(await screen.findByTestId("workarea-error")).toBeOnTheScreen();
  });

  it("calls onBack when the back control is tapped", async () => {
    const client = mockWorkspacesClient(demoWorkspacesFixture());
    const onBack = jest.fn();
    render(<WorkareaDetailScreen client={client} workspaceId="ws-web" onBack={onBack} />);

    // Let the workarea + its default segment settle before interacting, so no
    // async setState lands after the test (keeps the suite act()-clean).
    await screen.findByTestId("sessions-list");
    fireEvent.press(screen.getByTestId("workarea-back"));
    expect(onBack).toHaveBeenCalledTimes(1);
  });
});
