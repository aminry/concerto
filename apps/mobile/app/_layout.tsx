// Root layout (Task 508). expo-router file-system router (design/16 §3.1). The
// single Stack hosts the bottom-tab group `(tabs)`; settings/modals layer on as
// further routes in later Track-C tasks.
import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";

export default function RootLayout() {
  return (
    <>
      <StatusBar style="light" />
      <Stack screenOptions={{ headerShown: false }}>
        <Stack.Screen name="(tabs)" />
      </Stack>
    </>
  );
}
