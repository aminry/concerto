// Workspaces tab (Task 513) — the drill-down entry point (Workspace -> Workarea,
// NO project tier per D14). Mounts `WorkspacesScreen` over the app's
// WorkspacesClient seam and routes a tapped workspace to its workarea detail
// (`/workspace/[id]`). The seam is fixture-backed until the native transport
// lands (Task 510/516).
import { useRouter } from "expo-router";

import { WorkspacesScreen } from "../../src/workspaces/WorkspacesScreen";
import { appWorkspacesClient } from "../../src/data/app-client";

export default function WorkspacesTab() {
  const router = useRouter();
  return (
    <WorkspacesScreen
      client={appWorkspacesClient()}
      onOpenWorkspace={(ws) => router.push(`/workspace/${ws.id}`)}
    />
  );
}
