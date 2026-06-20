// Workarea detail screen (Task 513) — the drill-down target from the Workspaces
// list. Given a workspace id, it loads that workspace's workareas (Workspace ->
// Workarea, NO project tier per D14), defaults to the first, and renders a
// segmented **Sessions / Code & PRs** view over the [`WorkspacesClient`] seam.
//
// The segmented view is a JS-only Pressable `SegmentedControl` (Tier-2; a real
// swipe gesture is an allowed Tier-3 followup). Loading / empty / error states
// are handled per segment. Real @concerto/client generated types throughout.
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

import type { Workarea } from "@concerto/client/gen/concerto/v1/workareas_pb";
import type { Session } from "@concerto/client/gen/concerto/v1/sessions_pb";
import type { PullRequest } from "@concerto/client/gen/concerto/v1/vcs_pb";

import { colors, prStateColor, radius, spacing, statusColor } from "../theme/tokens";
import type { WorkspacesClient } from "../data/workspaces-client";
import { SegmentedControl } from "./SegmentedControl";
import { PrDiff } from "./PrDiff";

export interface WorkareaDetailScreenProps {
  client: WorkspacesClient;
  /** The workspace whose workarea we drilled into (from `/workspace/[id]`). */
  workspaceId: string;
  /** Back handler — the route file wires this to `router.back()`. */
  onBack?: () => void;
}

type Segment = "sessions" | "code";

type Phase<T> =
  | { phase: "loading" }
  | { phase: "error"; message: string }
  | { phase: "ready"; data: T };

function errMessage(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback;
}

function StatusPill({ status }: { status: string }) {
  const tint = statusColor(status);
  return (
    <View style={[styles.pill, { borderColor: tint }]}>
      <View style={[styles.pillDot, { backgroundColor: tint }]} />
      <Text style={[styles.pillText, { color: tint }]}>{status}</Text>
    </View>
  );
}

function SessionsSegment({
  client,
  workareaId,
}: {
  client: WorkspacesClient;
  workareaId: string;
}) {
  const [state, setState] = useState<Phase<Session[]>>({ phase: "loading" });

  useEffect(() => {
    let cancelled = false;
    setState({ phase: "loading" });
    client
      .listSessions(workareaId)
      .then((data) => !cancelled && setState({ phase: "ready", data }))
      .catch((err) =>
        !cancelled && setState({ phase: "error", message: errMessage(err, "Couldn't load sessions.") }),
      );
    return () => {
      cancelled = true;
    };
  }, [client, workareaId]);

  if (state.phase === "loading") {
    return (
      <View style={styles.segmentCenter} testID="sessions-loading">
        <ActivityIndicator color={colors.accent} />
      </View>
    );
  }
  if (state.phase === "error") {
    return (
      <View style={styles.segmentCenter} testID="sessions-error">
        <Text style={styles.errorText}>{state.message}</Text>
      </View>
    );
  }
  if (state.data.length === 0) {
    return (
      <View style={styles.segmentCenter} testID="sessions-empty">
        <Text style={styles.emptyText}>No sessions on this workarea yet.</Text>
      </View>
    );
  }
  return (
    <FlatList
      testID="sessions-list"
      data={state.data}
      keyExtractor={(s) => s.id}
      contentContainerStyle={styles.cardList}
      renderItem={({ item }) => (
        <View style={styles.card} testID={`session-${item.id}`}>
          <View style={styles.cardHead}>
            <Text style={styles.cardTitle}>{item.agentKind}</Text>
            <View style={styles.spacer} />
            <StatusPill status={item.status} />
          </View>
          {item.model ? <Text style={styles.cardSub}>{item.model}</Text> : null}
          <Text style={styles.cardMeta}>{item.id}</Text>
        </View>
      )}
    />
  );
}

function CodeSegment({
  client,
  workareaId,
}: {
  client: WorkspacesClient;
  workareaId: string;
}) {
  const [state, setState] = useState<Phase<PullRequest[]>>({ phase: "loading" });
  // Which PR's diff is expanded inline (Task 514). One open at a time.
  const [openPrId, setOpenPrId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setState({ phase: "loading" });
    client
      .getWorkareaPrSet(workareaId)
      .then((data) => !cancelled && setState({ phase: "ready", data }))
      .catch((err) =>
        !cancelled && setState({ phase: "error", message: errMessage(err, "Couldn't load pull requests.") }),
      );
    return () => {
      cancelled = true;
    };
  }, [client, workareaId]);

  if (state.phase === "loading") {
    return (
      <View style={styles.segmentCenter} testID="code-loading">
        <ActivityIndicator color={colors.accent} />
      </View>
    );
  }
  if (state.phase === "error") {
    return (
      <View style={styles.segmentCenter} testID="code-error">
        <Text style={styles.errorText}>{state.message}</Text>
      </View>
    );
  }
  if (state.data.length === 0) {
    return (
      <View style={styles.segmentCenter} testID="code-empty">
        <Text style={styles.emptyText}>No pull requests for this workarea yet.</Text>
      </View>
    );
  }
  return (
    <FlatList
      testID="code-list"
      data={state.data}
      keyExtractor={(pr) => pr.id}
      contentContainerStyle={styles.cardList}
      renderItem={({ item }) => {
        const tint = prStateColor(item.state);
        const expanded = openPrId === item.id;
        return (
          <View style={styles.card} testID={`pr-${item.id}`}>
            <Pressable
              testID={`pr-toggle-${item.id}`}
              onPress={() => setOpenPrId((prev) => (prev === item.id ? null : item.id))}
              accessibilityRole="button"
              accessibilityState={{ expanded }}
              accessibilityLabel={`${expanded ? "Hide" : "Show"} diff for PR ${item.title}`}
              style={({ pressed }) => [pressed && styles.pressed]}
            >
              <View style={styles.cardHead}>
                <Text style={styles.cardMeta}>#{item.prNumber.toString()}</Text>
                <View style={styles.spacer} />
                <View style={[styles.pill, { borderColor: tint }]}>
                  <Text style={[styles.pillText, { color: tint }]}>{item.state}</Text>
                </View>
              </View>
              <Text style={styles.cardTitle} numberOfLines={2}>
                {item.title}
              </Text>
              <Text style={styles.cardSub} numberOfLines={1}>
                {item.repositoryFullName} · {item.headRef} → {item.baseRef}
              </Text>
              <Text style={styles.diffToggleHint}>
                {expanded ? "▾ Hide diff" : "▸ View diff"}
              </Text>
            </Pressable>
            {expanded ? <PrDiff client={client} prId={item.id} /> : null}
          </View>
        );
      }}
    />
  );
}

export function WorkareaDetailScreen({
  client,
  workspaceId,
  onBack,
}: WorkareaDetailScreenProps) {
  const [state, setState] = useState<Phase<Workarea[]>>({ phase: "loading" });
  const [activeId, setActiveId] = useState<string | null>(null);
  const [segment, setSegment] = useState<Segment>("sessions");

  const load = useCallback(() => {
    let cancelled = false;
    setState({ phase: "loading" });
    client
      .listWorkareas(workspaceId)
      .then((data) => {
        if (cancelled) return;
        setState({ phase: "ready", data });
        setActiveId((prev) => prev ?? data[0]?.id ?? null);
      })
      .catch((err) => {
        if (!cancelled)
          setState({ phase: "error", message: errMessage(err, "Couldn't load workareas.") });
      });
    return () => {
      cancelled = true;
    };
  }, [client, workspaceId]);

  useEffect(() => load(), [load]);

  const active =
    state.phase === "ready" ? state.data.find((w) => w.id === activeId) ?? null : null;

  return (
    <SafeAreaView
      style={styles.screen}
      edges={["top", "left", "right"]}
      testID="workarea-detail-screen"
    >
      <View style={styles.header}>
        <Pressable
          testID="workarea-back"
          onPress={onBack}
          accessibilityRole="button"
          accessibilityLabel="Back to workspaces"
          style={({ pressed }) => [styles.backBtn, pressed && styles.pressed]}
        >
          <Text style={styles.backText}>‹ Workspaces</Text>
        </Pressable>
      </View>

      {state.phase === "loading" ? (
        <View style={styles.segmentCenter} testID="workarea-loading">
          <ActivityIndicator color={colors.accent} />
          <Text style={styles.emptyText}>Loading workarea…</Text>
        </View>
      ) : state.phase === "error" ? (
        <View style={styles.segmentCenter} testID="workarea-error">
          <Text style={styles.errorText}>{state.message}</Text>
          <Pressable
            testID="workarea-retry"
            onPress={load}
            accessibilityRole="button"
            accessibilityLabel="Retry loading workarea"
            style={({ pressed }) => [styles.retry, pressed && styles.pressed]}
          >
            <Text style={styles.retryText}>Try again</Text>
          </Pressable>
        </View>
      ) : !active ? (
        <View style={styles.segmentCenter} testID="workarea-empty">
          <Text style={styles.emptyText}>This workspace has no workareas yet.</Text>
        </View>
      ) : (
        <>
          <Text style={styles.title} accessibilityRole="header">
            {active.composerName}
          </Text>
          <View style={styles.subRow}>
            <Text style={styles.branch} numberOfLines={1}>
              {active.branchName}
            </Text>
            <StatusPill status={active.status} />
          </View>

          {state.data.length > 1 ? (
            <View style={styles.workareaSwitch} testID="workarea-switch">
              {state.data.map((wa) => {
                const selected = wa.id === active.id;
                return (
                  <Pressable
                    key={wa.id}
                    testID={`workarea-pick-${wa.id}`}
                    onPress={() => setActiveId(wa.id)}
                    accessibilityRole="button"
                    accessibilityState={{ selected }}
                    accessibilityLabel={`Workarea ${wa.composerName}`}
                    style={({ pressed }) => [
                      styles.chip,
                      selected && styles.chipSelected,
                      pressed && styles.pressed,
                    ]}
                  >
                    <Text style={[styles.chipText, selected && styles.chipTextSelected]}>
                      {wa.composerName}
                    </Text>
                  </Pressable>
                );
              })}
            </View>
          ) : null}

          <View style={styles.segmentWrap}>
            <SegmentedControl<Segment>
              options={[
                { value: "sessions", label: "Sessions" },
                { value: "code", label: "Code & PRs" },
              ]}
              value={segment}
              onChange={setSegment}
              testIDPrefix="seg"
            />
          </View>

          <View style={styles.segmentContent}>
            {segment === "sessions" ? (
              <SessionsSegment client={client} workareaId={active.id} />
            ) : (
              <CodeSegment client={client} workareaId={active.id} />
            )}
          </View>
        </>
      )}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: colors.bg,
    paddingHorizontal: spacing.lg,
  },
  header: {
    minHeight: 44,
    justifyContent: "center",
    marginTop: spacing.xs,
  },
  backBtn: {
    alignSelf: "flex-start",
    minHeight: 44,
    justifyContent: "center",
    paddingRight: spacing.md,
  },
  backText: {
    color: colors.accent,
    fontSize: 16,
    fontWeight: "600",
  },
  pressed: {
    opacity: 0.6,
  },
  title: {
    color: colors.text,
    fontSize: 24,
    fontWeight: "700",
    marginTop: spacing.xs,
  },
  subRow: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    marginTop: spacing.xs,
  },
  branch: {
    flex: 1,
    color: colors.textMuted,
    fontSize: 13,
  },
  workareaSwitch: {
    flexDirection: "row",
    flexWrap: "wrap",
    gap: spacing.xs,
    marginTop: spacing.md,
  },
  chip: {
    minHeight: 44,
    justifyContent: "center",
    paddingHorizontal: spacing.md,
    borderRadius: radius.sm,
    backgroundColor: colors.surface,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
  },
  chipSelected: {
    backgroundColor: colors.surfaceAlt,
    borderColor: colors.accent,
  },
  chipText: {
    color: colors.textMuted,
    fontSize: 13,
    fontWeight: "600",
  },
  chipTextSelected: {
    color: colors.text,
  },
  segmentWrap: {
    marginTop: spacing.md,
  },
  segmentContent: {
    flex: 1,
    marginTop: spacing.md,
  },
  segmentCenter: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    gap: spacing.sm,
    paddingBottom: spacing.xl,
  },
  cardList: {
    gap: spacing.sm,
    paddingBottom: spacing.xl,
  },
  card: {
    backgroundColor: colors.surface,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: radius.md,
    padding: spacing.md,
  },
  cardHead: {
    flexDirection: "row",
    alignItems: "center",
    marginBottom: spacing.xs,
  },
  spacer: {
    flex: 1,
  },
  cardTitle: {
    color: colors.text,
    fontSize: 15,
    fontWeight: "600",
    textTransform: "capitalize",
  },
  cardSub: {
    color: colors.textMuted,
    fontSize: 13,
    marginTop: spacing.xs / 2,
  },
  cardMeta: {
    color: colors.textMuted,
    fontSize: 12,
    fontVariant: ["tabular-nums"],
  },
  diffToggleHint: {
    color: colors.accent,
    fontSize: 13,
    fontWeight: "600",
    marginTop: spacing.sm,
    minHeight: 24,
  },
  pill: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.xs / 2,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: radius.sm,
    paddingHorizontal: spacing.sm,
    paddingVertical: 2,
  },
  pillDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
  },
  pillText: {
    fontSize: 12,
    fontWeight: "600",
    textTransform: "capitalize",
  },
  emptyText: {
    color: colors.textMuted,
    fontSize: 14,
    textAlign: "center",
  },
  errorText: {
    color: colors.danger,
    fontSize: 14,
    textAlign: "center",
  },
  retry: {
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
