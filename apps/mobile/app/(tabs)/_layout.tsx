// Bottom-tab navigation (Task 508 shell; Task 512 fills it in). Tab order is
// FROZEN by PHASE5_PLANNING D14 / design/16 §3.4: **Concerto** (default landing)
// — **Workspaces** — **Inbox**. "Concerto" is the user-facing name for the chat
// (Maestro is the internal service; desktop already renamed it).
import { Tabs } from "expo-router";

import { colors } from "../../src/theme/tokens";

export default function TabsLayout() {
  return (
    <Tabs
      screenOptions={{
        headerShown: false,
        tabBarActiveTintColor: colors.accent,
        tabBarInactiveTintColor: colors.textMuted,
        tabBarStyle: {
          backgroundColor: colors.surface,
          borderTopColor: colors.border,
        },
      }}
    >
      {/* Concerto chat is the default landing (the mobile inversion, design/16 §3.4). */}
      <Tabs.Screen name="index" options={{ title: "Concerto" }} />
      <Tabs.Screen name="workspaces" options={{ title: "Workspaces" }} />
      <Tabs.Screen name="inbox" options={{ title: "Inbox" }} />
    </Tabs>
  );
}
