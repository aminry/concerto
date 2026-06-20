// The Workspaces list screen (Task 513) — the drill-down entry point. Lists
// workspaces from the [`WorkspacesClient`] seam (real @concerto/client generated
// `Workspace` types via a mock client in tests / the app shell) with proper
// loading / empty / error states. Tapping a row drills into that workspace's
// workarea detail (Workspace -> Workarea, NO project tier per D14).
//
// Accessible + modern: a11y labels + roles, touch targets >= 44pt, dark-first
// tokens, and a `SafeAreaView` so content clears the notch / status bar.
import { useCallback, useEffect, useState } from "react";
import {
  ActivityIndicator,
  FlatList,
  Pressable,
  StyleSheet,
  Text,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import type { Workspace } from "@concerto/client/gen/concerto/v1/workspaces_pb";

import { colors, radius, spacing } from "../theme/tokens";
import type { WorkspacesClient } from "../data/workspaces-client";

export interface WorkspacesScreenProps {
  /** The data seam. Tests pass a `mockWorkspacesClient(...)`; the app passes the live one. */
  client: WorkspacesClient;
  /** Drill-down handler: the route file wires this to `router.push("/workspace/<id>")`. */
  onOpenWorkspace?: (workspace: Workspace) => void;
}

type LoadState =
  | { phase: "loading" }
  | { phase: "error"; message: string }
  | { phase: "ready"; workspaces: Workspace[] };

function WorkspaceRow({
  workspace,
  onPress,
}: {
  workspace: Workspace;
  onPress: () => void;
}) {
  return (
    <Pressable
      testID={`workspace-row-${workspace.id}`}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={`Open workspace ${workspace.name}`}
      accessibilityHint="Shows this workspace's workarea, sessions, and pull requests"
      style={({ pressed }) => [styles.row, pressed && styles.rowPressed]}
    >
      <Text style={styles.icon} accessibilityElementsHidden importantForAccessibility="no">
        {workspace.icon || "📁"}
      </Text>
      <View style={styles.rowBody}>
        <Text style={styles.rowTitle} numberOfLines={1}>
          {workspace.name}
        </Text>
        {workspace.description ? (
          <Text style={styles.rowSub} numberOfLines={2}>
            {workspace.description}
          </Text>
        ) : (
          <Text style={styles.rowSub} numberOfLines={1}>
            {workspace.slug}
          </Text>
        )}
      </View>
      <Text style={styles.chevron} accessibilityElementsHidden importantForAccessibility="no">
        ›
      </Text>
    </Pressable>
  );
}

export function WorkspacesScreen({ client, onOpenWorkspace }: WorkspacesScreenProps) {
  const [state, setState] = useState<LoadState>({ phase: "loading" });

  const load = useCallback(() => {
    let cancelled = false;
    setState({ phase: "loading" });
    client
      .listWorkspaces()
      .then((workspaces) => {
        if (!cancelled) setState({ phase: "ready", workspaces });
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setState({
            phase: "error",
            message: err instanceof Error ? err.message : "Couldn't load workspaces.",
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [client]);

  useEffect(() => load(), [load]);

  return (
    <SafeAreaView style={styles.screen} edges={["top", "left", "right"]} testID="workspaces-screen">
      <Text style={styles.title} accessibilityRole="header">
        Workspaces
      </Text>

      {state.phase === "loading" ? (
        <View style={styles.center} testID="workspaces-loading">
          <ActivityIndicator color={colors.accent} />
          <Text style={styles.centerSub}>Loading workspaces…</Text>
        </View>
      ) : state.phase === "error" ? (
        <View style={styles.center} testID="workspaces-error">
          <Text style={styles.errorTitle}>Couldn&rsquo;t load workspaces</Text>
          <Text style={styles.centerSub}>{state.message}</Text>
          <Pressable
            testID="workspaces-retry"
            onPress={load}
            accessibilityRole="button"
            accessibilityLabel="Retry loading workspaces"
            style={({ pressed }) => [styles.retry, pressed && styles.rowPressed]}
          >
            <Text style={styles.retryText}>Try again</Text>
          </Pressable>
        </View>
      ) : state.workspaces.length === 0 ? (
        <View style={styles.center} testID="workspaces-empty">
          <Text style={styles.emptyTitle}>No workspaces yet</Text>
          <Text style={styles.centerSub}>
            Create a workspace on your Core and it&rsquo;ll show up here.
          </Text>
        </View>
      ) : (
        <FlatList
          testID="workspaces-list"
          data={state.workspaces}
          keyExtractor={(w) => w.id}
          renderItem={({ item }) => (
            <WorkspaceRow workspace={item} onPress={() => onOpenWorkspace?.(item)} />
          )}
          contentContainerStyle={styles.list}
        />
      )}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: colors.bg,
    paddingHorizontal: spacing.lg,
    paddingTop: spacing.lg,
  },
  title: {
    color: colors.text,
    fontSize: 22,
    fontWeight: "700",
    marginBottom: spacing.md,
  },
  list: {
    gap: spacing.sm,
    paddingBottom: spacing.xl,
  },
  row: {
    flexDirection: "row",
    alignItems: "center",
    minHeight: 64,
    backgroundColor: colors.surface,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: radius.md,
    paddingHorizontal: spacing.md,
    paddingVertical: spacing.md,
    gap: spacing.md,
  },
  rowPressed: {
    backgroundColor: colors.surfaceAlt,
  },
  icon: {
    fontSize: 22,
  },
  rowBody: {
    flex: 1,
  },
  rowTitle: {
    color: colors.text,
    fontSize: 16,
    fontWeight: "600",
  },
  rowSub: {
    color: colors.textMuted,
    fontSize: 13,
    marginTop: spacing.xs / 2,
  },
  chevron: {
    color: colors.textMuted,
    fontSize: 24,
    fontWeight: "400",
  },
  center: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    paddingBottom: spacing.xl,
    gap: spacing.xs,
  },
  centerSub: {
    color: colors.textMuted,
    fontSize: 13,
    textAlign: "center",
    marginTop: spacing.xs,
  },
  emptyTitle: {
    color: colors.text,
    fontSize: 16,
    fontWeight: "600",
  },
  errorTitle: {
    color: colors.danger,
    fontSize: 16,
    fontWeight: "600",
  },
  retry: {
    marginTop: spacing.md,
    minHeight: 44,
    justifyContent: "center",
    paddingHorizontal: spacing.lg,
    borderRadius: radius.sm,
    backgroundColor: colors.surfaceAlt,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
  },
  retryText: {
    color: colors.text,
    fontSize: 14,
    fontWeight: "600",
  },
});
