// @vitest-environment jsdom
//
// Component tests for the desktop notifications-inbox surface (Task 523). Proves
// the desktop shell renders the SHARED `@concerto/ui` `Inbox` — the idle surface
// by default, a severity-coded feed when fed notifications, and an interactive
// unread-only toggle. The shared component is the same one apps/web renders, so
// this is the desktop half of the "one inbox, two hosts" guarantee.

import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { create } from "@bufbuild/protobuf";
import {
  NotificationKind,
  NotificationSchema,
} from "@concerto/client/gen/concerto/v1/notifications_pb";

import { InboxPanel } from "./InboxPanel";

describe("InboxPanel", () => {
  it("renders the shared inbox idle surface by default", () => {
    render(<InboxPanel />);
    expect(screen.getByTestId("desktop-inbox")).toBeInTheDocument();
    // The shared @concerto/ui component, mounted inside the desktop shell.
    expect(screen.getByTestId("inbox")).toBeInTheDocument();
    expect(screen.getByTestId("idle")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Notifications" })).toBeInTheDocument();
  });

  it("renders a severity-coded feed when fed notifications", () => {
    const items = [
      create(NotificationSchema, {
        id: "n1",
        kind: NotificationKind.TOOL_APPROVAL_NEEDED,
        title: "Approve shell command",
        body: "rm -rf build/",
        severity: "high",
        createdAtMs: BigInt(Date.now()),
      }),
    ];
    render(<InboxPanel items={items} status={{ kind: "ok", count: items.length }} />);
    expect(screen.getByTestId("feed")).toBeInTheDocument();
    expect(screen.getByText("Approve shell command")).toBeInTheDocument();
    // The kind label comes from the shared renderer's KIND_LABEL map.
    expect(screen.getByText("Approval needed")).toBeInTheDocument();
  });

  it("toggling 'unread only' flips the shared filter checkbox", async () => {
    render(<InboxPanel />);
    const toggle = screen.getByTestId("unread-toggle");
    expect(toggle).not.toBeChecked();
    await userEvent.click(toggle);
    expect(toggle).toBeChecked();
  });
});
