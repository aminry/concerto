// Desktop notifications-inbox surface (Task 523 — decision D11).
//
// Renders the SHARED `@concerto/ui` `Inbox` inside the desktop shell so the
// desktop and web clients show the identical inbox (severity-coded cards, the
// unread-only filter, the idle/empty/error surfaces). The shared component is
// transport-agnostic: this panel owns the load state + the handlers, the same
// way `apps/web` owns the connect-web fetch.
//
// The desktop → Core transport for the live `Notifications` service flows
// through the existing `concerto_rpc` Tauri bridge (`src/api`), not yet through
// `@concerto/client` (that migration is a follow-up — see D10). Until it is
// wired, this panel mounts the shared inbox in its idle state and accepts the
// feed + handlers as props so the parent (or a test) can drive it. The
// component proves desktop renders `@concerto/ui` end to end; populating it from
// the live service is the next desktop step.

import { useState } from "react";

import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { Inbox, type InboxStatus } from "@concerto/ui";

// The shared inbox styles. Co-located in `@concerto/ui` and imported once here so
// the desktop surface themes consistently with the web client.
import "@concerto/ui/inbox.css";

export interface InboxPanelProps {
  /** The notification feed (newest-first). Defaults to empty until the live
      service is wired through the `concerto_rpc` bridge. */
  items?: Notification[];
  /** Load state; defaults to idle ("Connect to a Core"). */
  status?: InboxStatus;
  /** Mark a notification read by id (no-op until the live service is wired). */
  onMarkRead?: (id: string) => void;
}

/** The desktop inbox panel — the shared `@concerto/ui` `Inbox` in a scrollable
    surface that matches the right-rail tab chrome. */
export function InboxPanel({
  items = [],
  status = { kind: "idle" },
  onMarkRead = () => {},
}: InboxPanelProps): JSX.Element {
  // The unread-only filter is local UI state; once the live service is wired it
  // will drive a refetch (mirroring the web shell's `onUnreadOnlyChange`).
  const [unreadOnly, setUnreadOnly] = useState(false);

  return (
    <div className="h-full overflow-y-auto p-4" data-testid="desktop-inbox">
      <Inbox
        items={items}
        status={status}
        unreadOnly={unreadOnly}
        onUnreadOnlyChange={setUnreadOnly}
        onMarkRead={onMarkRead}
      />
    </div>
  );
}
