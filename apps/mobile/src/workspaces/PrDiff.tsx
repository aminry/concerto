// PR diff drill-down (Task 514). Loads a PR's unified-diff text through the
// `WorkspacesClient.getPrDiff` seam and renders it with the pure-RN `DiffView`.
// Mounted inline (expanded) under a tapped PR card in the Code & PRs segment.
// Loading / error / empty states mirror the rest of the drill-down. Tier-2: the
// diff text is a typed fixture/mock; the on-device perf budget is Tier-3.
import { useEffect, useState } from "react";
import { ActivityIndicator, StyleSheet, Text, View } from "react-native";

import { colors, radius, spacing } from "../theme/tokens";
import type { WorkspacesClient } from "../data/workspaces-client";
import { DiffView } from "../diff/DiffView";

export interface PrDiffProps {
  client: WorkspacesClient;
  prId: string;
}

type Phase =
  | { phase: "loading" }
  | { phase: "error"; message: string }
  | { phase: "ready"; diff: string };

export function PrDiff({ client, prId }: PrDiffProps) {
  const [state, setState] = useState<Phase>({ phase: "loading" });

  useEffect(() => {
    let cancelled = false;
    setState({ phase: "loading" });
    client
      .getPrDiff(prId)
      .then((diff) => !cancelled && setState({ phase: "ready", diff }))
      .catch(
        (err) =>
          !cancelled &&
          setState({
            phase: "error",
            message: err instanceof Error ? err.message : "Couldn't load the diff.",
          }),
      );
    return () => {
      cancelled = true;
    };
  }, [client, prId]);

  if (state.phase === "loading") {
    return (
      <View style={styles.center} testID={`pr-diff-loading-${prId}`}>
        <ActivityIndicator color={colors.accent} />
      </View>
    );
  }
  if (state.phase === "error") {
    return (
      <View style={styles.center} testID={`pr-diff-error-${prId}`}>
        <Text style={styles.errorText}>{state.message}</Text>
      </View>
    );
  }
  if (state.diff.trim() === "") {
    return (
      <View style={styles.center} testID={`pr-diff-empty-${prId}`}>
        <Text style={styles.emptyText}>No diff available for this PR.</Text>
      </View>
    );
  }
  return (
    <View style={styles.diffWrap} testID={`pr-diff-${prId}`}>
      <DiffView diffText={state.diff} testID={`pr-diff-view-${prId}`} />
    </View>
  );
}

const styles = StyleSheet.create({
  diffWrap: {
    // A bounded height so the inner FlatList virtualizes inside the card list
    // (an unbounded child of a parent FlatList can't scroll independently).
    height: 320,
    marginTop: spacing.sm,
    borderRadius: radius.sm,
    overflow: "hidden",
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
  },
  center: {
    minHeight: 80,
    alignItems: "center",
    justifyContent: "center",
    marginTop: spacing.sm,
    gap: spacing.sm,
  },
  errorText: {
    color: colors.danger,
    fontSize: 13,
    textAlign: "center",
  },
  emptyText: {
    color: colors.textMuted,
    fontSize: 13,
    textAlign: "center",
  },
});
