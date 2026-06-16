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

  it("renders an announced loading surface while the first fetch is in flight", () => {
    // On a host's first connect the status is { kind: "loading" } with no items
    // yet — the shared inbox must show a Loading… surface (not a dead blank
    // panel), and it must be a polite live region for screen readers.
    render(<InboxPanel status={{ kind: "loading" }} />);
    const loading = screen.getByTestId("loading");
    expect(loading).toBeInTheDocument();
    expect(loading).toHaveTextContent("Loading…");
    expect(loading).toHaveAttribute("role", "status");
    expect(loading).toHaveAttribute("aria-live", "polite");
    // An in-flight refetch must not blank an idle/empty surface as well.
    expect(screen.queryByTestId("idle")).not.toBeInTheDocument();
  });

  it("keeps showing the existing feed during an in-place refetch", () => {
    // A refetch over an already-loaded feed (loading + items present) must keep
    // the current list visible rather than swap it for the Loading… surface.
    const items = [
      create(NotificationSchema, {
        id: "n1",
        kind: NotificationKind.PR_STATE_CHANGED,
        title: "PR #7 merged",
        severity: "low",
        createdAtMs: BigInt(Date.now()),
      }),
    ];
    render(<InboxPanel items={items} status={{ kind: "loading" }} />);
    expect(screen.getByTestId("feed")).toBeInTheDocument();
    expect(screen.getByText("PR #7 merged")).toBeInTheDocument();
    expect(screen.queryByTestId("loading")).not.toBeInTheDocument();
  });

  it("normalizes an unknown wire severity to the 'low' bucket", () => {
    // `severity` is a free-form wire string; an unexpected value ("critical")
    // must collapse to a defined bucket so the card stays styled and the raw
    // string is never injected as the pill text.
    const items = [
      create(NotificationSchema, {
        id: "n2",
        kind: NotificationKind.AGENT_CRASHED,
        title: "Agent died",
        severity: "critical",
        createdAtMs: BigInt(Date.now()),
      }),
    ];
    render(<InboxPanel items={items} status={{ kind: "ok", count: items.length }} />);
    const card = screen.getByTestId("notification");
    // Normalized className + pill, never the raw "critical".
    expect(card.className).toContain("sev-low");
    expect(card.className).not.toContain("sev-critical");
    expect(screen.getByText("low")).toBeInTheDocument();
    expect(screen.queryByText("critical")).not.toBeInTheDocument();
  });

  it("announces the idle and 'all caught up' surfaces as live regions", () => {
    // idle (default) and the filter-emptied 'all caught up' state must be polite
    // live regions so a screen-reader user gets feedback when the list changes.
    const { rerender } = render(<InboxPanel />);
    const idle = screen.getByTestId("idle");
    expect(idle).toHaveAttribute("role", "status");
    expect(idle).toHaveAttribute("aria-live", "polite");

    rerender(<InboxPanel items={[]} status={{ kind: "ok", count: 0 }} />);
    const empty = screen.getByTestId("empty");
    expect(empty).toHaveAttribute("role", "status");
    expect(empty).toHaveAttribute("aria-live", "polite");
  });
});
