// Inbox tab — renders the notifications feed (Task 508). Wired to
// @concerto/client's generated `Notification` type via `InboxScreen`. The live
// feed (over the native ConcertoIroh DataClient) lands in Task 510/516; the
// scaffold mounts the empty-state shell.
import { InboxScreen } from "../../src/inbox/InboxScreen";

export default function InboxTab() {
  return <InboxScreen />;
}
