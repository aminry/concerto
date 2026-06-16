// Root layout (Task 508). expo-router file-system router (design/16 §3.1). The
// single Stack hosts the bottom-tab group `(tabs)` plus the Workspaces
// drill-down sub-screen `workspace/[id]` (Task 513); settings/modals layer on as
// further routes in later Track-C tasks.
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
      </Stack>
    </SafeAreaProvider>
  );
}
