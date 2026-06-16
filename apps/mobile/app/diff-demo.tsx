// Diff renderer demo route (Task 514). Showcases the pure-RN `DiffView` over the
// representative `SAMPLE_DIFF` fixture: tap a hunk header to collapse/expand,
// scroll a long line horizontally. Reachable at `/diff-demo` (registered in
// app/_layout.tsx). The spike-103 60fps / <1.5s budget is a Tier-3 on-device
// line surfaced in the footer — it is NOT measured in this shell.
import { Pressable, StyleSheet, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { useRouter } from "expo-router";

import { DiffView, PERF_BUDGET } from "../src/diff/DiffView";
import { SAMPLE_DIFF } from "../src/diff/diff-fixtures";
import { colors, radius, spacing } from "../src/theme/tokens";

export default function DiffDemoRoute() {
  const router = useRouter();
  return (
    <SafeAreaView style={styles.screen} edges={["top", "left", "right"]} testID="diff-demo-screen">
      <View style={styles.header}>
        <Pressable
          testID="diff-demo-back"
          onPress={() => router.back()}
          accessibilityRole="button"
          accessibilityLabel="Back"
          style={({ pressed }) => [styles.backBtn, pressed && styles.pressed]}
        >
          <Text style={styles.backText}>‹ Back</Text>
        </Pressable>
        <Text style={styles.title} accessibilityRole="header">
          Diff renderer
        </Text>
      </View>

      <View style={styles.diffWrap}>
        <DiffView diffText={SAMPLE_DIFF} />
      </View>

      <Text style={styles.note}>
        Tap a hunk header to collapse it. Long lines scroll sideways. Budget:{" "}
        {PERF_BUDGET.diffLines}-line diff &lt; {PERF_BUDGET.firstPaintMs / 1000}s /{" "}
        {PERF_BUDGET.targetFps}fps on {PERF_BUDGET.devices.join(" / ")} (verified
        on-device — Tier-3).
      </Text>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: colors.bg,
  },
  header: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    paddingHorizontal: spacing.lg,
    minHeight: 44,
  },
  backBtn: {
    minHeight: 44,
    justifyContent: "center",
    paddingRight: spacing.sm,
  },
  backText: {
    color: colors.accent,
    fontSize: 16,
    fontWeight: "600",
  },
  pressed: { opacity: 0.6 },
  title: {
    color: colors.text,
    fontSize: 20,
    fontWeight: "700",
  },
  diffWrap: {
    flex: 1,
    marginTop: spacing.sm,
    marginHorizontal: spacing.md,
    borderRadius: radius.md,
    overflow: "hidden",
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
  },
  note: {
    color: colors.textMuted,
    fontSize: 12,
    padding: spacing.md,
  },
});
