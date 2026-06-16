// The app's WorkspacesClient instance (Task 513). For now this is the
// fixture-backed mock so the drill-down renders a representative feed in the app
// shell (pre-live-transport). Task 510/516 swaps this for a transport-backed
// implementation over @concerto/client's native `DataClient` — the screens take
// the seam as a prop, so only this factory changes.
import { mockWorkspacesClient, type WorkspacesClient } from "./workspaces-client";
import { demoWorkspacesFixture } from "./fixtures";

let cached: WorkspacesClient | undefined;

/** The app-wide WorkspacesClient (memoised so screens share one fixture set). */
export function appWorkspacesClient(): WorkspacesClient {
  if (!cached) {
    cached = mockWorkspacesClient(demoWorkspacesFixture());
  }
  return cached;
}
