// The "Pair a Core" entry affordance (Task 511). A small pair of buttons the app
// shell surfaces (e.g. on the Concerto landing tab) so the user can start the
// pairing flow or manage their paired Cores. Navigation is injected so the
// component stays Tier-2-testable and route-agnostic.
import { Pressable, StyleSheet, Text, View } from "react-native";

import { colors, radius, spacing } from "../theme/tokens";

export interface PairEntryProps {
  /** Open the QR scanner (the host routes to `/pair`). */
  onPair: () => void;
  /** Open the multi-Core picker (the host routes to `/cores`). */
  onManageCores?: () => void;
}

export function PairEntry({ onPair, onManageCores }: PairEntryProps) {
  return (
    <View style={styles.wrap} testID="pair-entry">
      <Pressable
        testID="pair-entry-pair"
        onPress={onPair}
        accessibilityRole="button"
        accessibilityLabel="Pair a Core"
        style={({ pressed }) => [styles.primaryBtn, pressed && styles.pressed]}
      >
        <Text style={styles.primaryBtnText}>Pair a Core</Text>
      </Pressable>
      {onManageCores ? (
        <Pressable
          testID="pair-entry-manage"
          onPress={onManageCores}
          accessibilityRole="button"
          accessibilityLabel="Manage paired Cores"
          style={({ pressed }) => [styles.linkBtn, pressed && styles.pressed]}
        >
          <Text style={styles.linkText}>Manage Cores</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: {
    alignItems: "center",
    gap: spacing.sm,
    marginTop: spacing.lg,
  },
  primaryBtn: {
    minHeight: 48,
    justifyContent: "center",
    alignItems: "center",
    paddingHorizontal: spacing.xl,
    borderRadius: radius.md,
    backgroundColor: colors.accent,
  },
  primaryBtnText: { color: "#0b0e14", fontSize: 16, fontWeight: "700" },
  linkBtn: { minHeight: 44, justifyContent: "center" },
  linkText: { color: colors.accent, fontSize: 15 },
  pressed: { opacity: 0.6 },
});
