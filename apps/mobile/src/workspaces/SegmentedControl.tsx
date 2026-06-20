// A JS-only segmented control (Task 513). Pressable-based, so it stays a Tier-2
// jest test (NO native-only deps like react-native-pager-view / gesture-handler
// that would force `expo prebuild` => Tier-3). A real swipe gesture between the
// segments is an allowed Tier-3 followup; this control switches on tap.
//
// Accessible: each segment is a `tab` with `selected` state + a touch target
// >= 44pt (the row is `minHeight: 44`).
import { Pressable, StyleSheet, Text, View } from "react-native";

import { colors, radius, spacing } from "../theme/tokens";

export interface SegmentOption<T extends string> {
  value: T;
  label: string;
}

export interface SegmentedControlProps<T extends string> {
  options: SegmentOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** Optional testID prefix; each segment gets `<prefix>-<value>`. */
  testIDPrefix?: string;
}

export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  testIDPrefix = "segment",
}: SegmentedControlProps<T>) {
  return (
    <View style={styles.track} accessibilityRole="tablist">
      {options.map((opt) => {
        const selected = opt.value === value;
        return (
          <Pressable
            key={opt.value}
            testID={`${testIDPrefix}-${opt.value}`}
            onPress={() => onChange(opt.value)}
            accessibilityRole="tab"
            accessibilityLabel={opt.label}
            accessibilityState={{ selected }}
            style={[styles.segment, selected && styles.segmentSelected]}
          >
            <Text style={[styles.label, selected && styles.labelSelected]} numberOfLines={1}>
              {opt.label}
            </Text>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  track: {
    flexDirection: "row",
    backgroundColor: colors.surfaceAlt,
    borderRadius: radius.md,
    padding: spacing.xs / 2,
    gap: spacing.xs / 2,
  },
  segment: {
    flex: 1,
    minHeight: 44,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: radius.sm,
    paddingHorizontal: spacing.sm,
  },
  segmentSelected: {
    backgroundColor: colors.surface,
  },
  label: {
    color: colors.textMuted,
    fontSize: 14,
    fontWeight: "600",
  },
  labelSelected: {
    color: colors.text,
  },
});
