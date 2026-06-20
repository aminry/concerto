// Root layout (Task 508 + Task 511 + Tasks 516/518). expo-router file-system
// router (design/16 §3.1). The single Stack hosts the bottom-tab group `(tabs)`,
// the Workspaces drill-down sub-screen `workspace/[id]` (Task 513), and the
// pairing flow (`pair`) + multi-Core picker (`cores`) (Task 511). `pair` presents
// modally so it overlays the tab bar during the camera scan.
//
// `useAppLifecycle` (Task 518) drives the background/foreground session lifecycle:
// on foreground it opens the native session, resubscribes streams from their
// since_offset, and registers for push (Task 516); on background it closes the
// session. The controller is built once per app launch.
import { useMemo } from "react";
import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { createAppSession } from "../src/lifecycle/app-session";
import { useAppLifecycle } from "../src/lifecycle/app-lifecycle";

export default function RootLayout() {
  const { controller } = useMemo(() => createAppSession(), []);
  useAppLifecycle(controller);

  return (
    <SafeAreaProvider>
      <StatusBar style="light" />
      <Stack screenOptions={{ headerShown: false }}>
        <Stack.Screen name="(tabs)" />
        <Stack.Screen name="workspace/[id]" />
        <Stack.Screen name="pair" options={{ presentation: "modal" }} />
        <Stack.Screen name="cores" options={{ presentation: "modal" }} />
        {/* Task 514 diff renderer demo + Task 517 localhost preview tunnel. */}
        <Stack.Screen name="diff-demo" />
        <Stack.Screen name="preview/[id]" />
      </Stack>
    </SafeAreaProvider>
  );
}
