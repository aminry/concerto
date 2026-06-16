// Workarea detail route (Task 513). `/workspace/<id>` is the drill-down target
// from the Workspaces list: it resolves the workspace's workarea(s) and renders
// the Sessions / Code & PRs segmented view (Workspace -> Workarea, NO project
// tier per D14). The route is keyed by workspace id; the screen picks the active
// workarea. Lives outside `(tabs)` so it pushes over the tab bar as a sub-screen.
import { useLocalSearchParams, useRouter } from "expo-router";

import { WorkareaDetailScreen } from "../../src/workspaces/WorkareaDetailScreen";
import { appWorkspacesClient } from "../../src/data/app-client";

export default function WorkspaceDetailRoute() {
  const router = useRouter();
  const { id } = useLocalSearchParams<{ id: string }>();
  return (
    <WorkareaDetailScreen
      client={appWorkspacesClient()}
      workspaceId={id ?? ""}
      onBack={() => router.back()}
    />
  );
}
