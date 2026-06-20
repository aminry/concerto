// Sample mobile unit test (Task 508): the Inbox screen renders. Proves the
// jest + jest-expo + @testing-library/react-native harness works AND that the
// fresh RN component tree consumes @concerto/client's generated `Notification`
// type (PHASE5_PLANNING D11 — mobile shares only @concerto/client).
import { render, screen } from "@testing-library/react-native";
import { create } from "@bufbuild/protobuf";

import {
  NotificationKind,
  NotificationSchema,
} from "@concerto/client/gen/concerto/v1/notifications_pb";

import { InboxScreen } from "./InboxScreen";

describe("InboxScreen", () => {
  it("renders the empty state when there are no notifications", () => {
    render(<InboxScreen />);
    expect(screen.getByTestId("inbox-screen")).toBeOnTheScreen();
    expect(screen.getByTestId("inbox-empty")).toBeOnTheScreen();
  });

  it("renders a notification card from a @concerto/client Notification", () => {
    const notif = create(NotificationSchema, {
      id: "01HZTESTULID",
      kind: NotificationKind.TOOL_APPROVAL_NEEDED,
      title: "Approve `rm -rf build/`",
      body: "Claude wants to run a destructive command.",
      severity: "high",
      createdAtMs: BigInt(Date.now()),
    });

    render(<InboxScreen items={[notif]} />);

    expect(screen.getByTestId("inbox-feed")).toBeOnTheScreen();
    expect(screen.getByTestId("notification")).toBeOnTheScreen();
    expect(screen.getByText("Approve `rm -rf build/`")).toBeOnTheScreen();
    // The kind label is derived from @concerto/client's generated enum.
    expect(screen.getByText("Approval needed")).toBeOnTheScreen();
  });
});
