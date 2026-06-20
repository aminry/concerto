// A minimal placeholder screen (Task 508) for tabs whose real surfaces land in
// later Track-C tasks: the Concerto chat (Task 512+) and Workspaces drill-down
// (Task 513). Fresh RN component tree (PHASE5_PLANNING D11).
import { StyleSheet, Text, View } from "react-native";

import { colors, spacing } from "./theme/tokens";

export interface PlaceholderProps {
  title: string;
  subtitle: string;
}

export function Placeholder({ title, subtitle }: PlaceholderProps) {
  return (
    <View style={styles.screen} testID="placeholder-screen">
      <Text style={styles.title}>{title}</Text>
      <Text style={styles.subtitle}>{subtitle}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: colors.bg,
    alignItems: "center",
    justifyContent: "center",
    padding: spacing.xl,
  },
  title: {
    color: colors.text,
    fontSize: 22,
    fontWeight: "700",
    marginBottom: spacing.sm,
  },
  subtitle: {
    color: colors.textMuted,
    fontSize: 14,
    textAlign: "center",
  },
});
