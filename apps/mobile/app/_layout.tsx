// Root layout (Task 508 + Task 511). expo-router file-system router (design/16
// §3.1). The single Stack hosts the bottom-tab group `(tabs)`, the Workspaces
// drill-down sub-screen `workspace/[id]` (Task 513), and the pairing flow
// (`pair`) + multi-Core picker (`cores`) (Task 511). `pair` presents modally so
// it overlays the tab bar during the camera scan.
import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import { SafeAreaProvider } from "react-native-safe-area-context";

export default function RootLayout() {
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
