// Localhost preview tunnel route (Task 517). `/preview/<id>` requests a public
// tunnel URL for the workarea/dev-server `id` and renders it in a WebView. Lives
// outside `(tabs)` so it pushes over the tab bar as a sub-screen. Registered in
// app/_layout.tsx.
import { useLocalSearchParams, useRouter } from "expo-router";

import { PreviewScreen } from "../../src/preview/PreviewScreen";
import { appTunnelClient } from "../../src/data/app-client";

export default function PreviewRoute() {
  const router = useRouter();
  const { id } = useLocalSearchParams<{ id: string }>();
  return (
    <PreviewScreen
      client={appTunnelClient()}
      id={id ?? ""}
      onBack={() => router.back()}
    />
  );
}
