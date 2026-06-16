// The multi-Core picker (Task 511; design/16 §3.6). Lists the paired Cores from
// the secure-store registry, marks the active one, and lets the user switch
// among them, remove one, or pair another. Same UX language as the other
// screens: dark tokens, a11y, >= 44pt targets, loading / empty states.
//
// The registry calls are injectable so the screen is Tier-2-testable without the
// (mocked) secure-store needing pre-seeding through the real module import path.
import { useCallback, useEffect, useState } from "react";
import { ActivityIndicator, FlatList, Pressable, StyleSheet, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import { colors, radius, spacing } from "../theme/tokens";
import {
  activeCoreId as defaultActiveCoreId,
  listCores as defaultListCores,
  removeCore as defaultRemoveCore,
  switchCore as defaultSwitchCore,
  type StoredCore,
} from "./core-store";

export interface CoresScreenProps {
  /** Pair-another affordance (the route pushes `/pair`). */
  onPairAnother?: () => void;
  /** Dismiss the picker (the route goes back). */
  onClose?: () => void;
  /** Injectable registry (defaults to the secure-store-backed `core-store`). */
  registry?: {
    listCores: () => Promise<StoredCore[]>;
    activeCoreId: () => Promise<string | null>;
    switchCore: (id: string) => Promise<void>;
    removeCore: (id: string) => Promise<void>;
  };
}

type LoadState =
  | { phase: "loading" }
  | { phase: "ready"; cores: StoredCore[]; activeId: string | null };

const defaultRegistry = {
  listCores: defaultListCores,
  activeCoreId: defaultActiveCoreId,
  switchCore: defaultSwitchCore,
  removeCore: defaultRemoveCore,
};

export function CoresScreen({ onPairAnother, onClose, registry }: CoresScreenProps) {
  const reg = registry ?? defaultRegistry;
  const [state, setState] = useState<LoadState>({ phase: "loading" });

  const load = useCallback(() => {
    let cancelled = false;
    setState({ phase: "loading" });
    Promise.all([reg.listCores(), reg.activeCoreId()])
      .then(([cores, activeId]) => {
        if (!cancelled) setState({ phase: "ready", cores, activeId });
      })
      .catch(() => {
        if (!cancelled) setState({ phase: "ready", cores: [], activeId: null });
      });
    return () => {
      cancelled = true;
    };
  }, [reg]);

  useEffect(() => load(), [load]);

  const onSwitch = useCallback(
    async (id: string) => {
      await reg.switchCore(id);
      load();
    },
    [reg, load],
  );

  const onRemove = useCallback(
    async (id: string) => {
      await reg.removeCore(id);
      load();
    },
    [reg, load],
  );

  return (
    <SafeAreaView style={styles.screen} edges={["top", "left", "right"]} testID="cores-screen">
      <View style={styles.header}>
        <Pressable
          testID="cores-close"
          onPress={onClose}
          accessibilityRole="button"
          accessibilityLabel="Close"
          style={({ pressed }) => [styles.backBtn, pressed && styles.pressed]}
        >
          <Text style={styles.backText}>‹ Back</Text>
        </Pressable>
        <Text style={styles.title} accessibilityRole="header">
          Cores
        </Text>
        <View style={styles.backBtn} />
      </View>

      {state.phase === "loading" ? (
        <View style={styles.center} testID="cores-loading">
          <ActivityIndicator color={colors.accent} />
        </View>
      ) : state.cores.length === 0 ? (
        <View style={styles.center} testID="cores-empty">
          <Text style={styles.emptyTitle}>No Cores paired yet</Text>
          <Text style={styles.centerSub}>Pair a Core to control it from your phone.</Text>
          <Pressable
            testID="cores-pair-empty"
            onPress={onPairAnother}
            accessibilityRole="button"
            accessibilityLabel="Pair a Core"
            style={({ pressed }) => [styles.primaryBtn, pressed && styles.pressed]}
          >
            <Text style={styles.primaryBtnText}>Pair a Core</Text>
          </Pressable>
        </View>
      ) : (
        <FlatList
          testID="cores-list"
          data={state.cores}
          keyExtractor={(c) => c.id}
          contentContainerStyle={styles.list}
          renderItem={({ item }) => {
            const active = item.id === state.activeId;
            return (
              <View testID={`core-row-${item.id}`} style={styles.row}>
                <Pressable
                  onPress={() => void onSwitch(item.id)}
                  accessibilityRole="button"
                  accessibilityState={{ selected: active }}
                  accessibilityLabel={`Use ${item.label}${active ? ", current Core" : ""}`}
                  style={styles.rowMain}
                >
                  <View style={[styles.dot, active && styles.dotActive]} />
                  <View style={styles.rowBody}>
                    <Text style={styles.rowTitle} numberOfLines={1}>
                      {item.label}
                    </Text>
                    <Text style={styles.rowSub} numberOfLines={1}>
                      {active ? "Active" : "Tap to switch"}
                    </Text>
                  </View>
                </Pressable>
                <Pressable
                  testID={`core-remove-${item.id}`}
                  onPress={() => void onRemove(item.id)}
                  accessibilityRole="button"
                  accessibilityLabel={`Remove ${item.label}`}
                  style={({ pressed }) => [styles.removeBtn, pressed && styles.pressed]}
                >
                  <Text style={styles.removeText}>Remove</Text>
                </Pressable>
              </View>
            );
          }}
          ListFooterComponent={
            <Pressable
              testID="cores-pair-another"
              onPress={onPairAnother}
              accessibilityRole="button"
              accessibilityLabel="Pair another Core"
              style={({ pressed }) => [styles.secondaryBtn, pressed && styles.pressed]}
            >
              <Text style={styles.secondaryBtnText}>+ Pair another Core</Text>
            </Pressable>
          }
        />
      )}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg, paddingHorizontal: spacing.lg },
  header: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingVertical: spacing.md,
  },
  backBtn: { minWidth: 64, minHeight: 44, justifyContent: "center" },
  backText: { color: colors.accent, fontSize: 16 },
  title: { color: colors.text, fontSize: 18, fontWeight: "700" },
  list: { gap: spacing.sm, paddingBottom: spacing.xl },
  row: {
    flexDirection: "row",
    alignItems: "center",
    minHeight: 64,
    backgroundColor: colors.surface,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: radius.md,
    paddingHorizontal: spacing.md,
  },
  rowMain: {
    flex: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    minHeight: 64,
  },
  dot: {
    width: 12,
    height: 12,
    borderRadius: 6,
    borderColor: colors.border,
    borderWidth: 1,
    backgroundColor: "transparent",
  },
  dotActive: { backgroundColor: colors.success, borderColor: colors.success },
  rowBody: { flex: 1 },
  rowTitle: { color: colors.text, fontSize: 16, fontWeight: "600" },
  rowSub: { color: colors.textMuted, fontSize: 13, marginTop: spacing.xs / 2 },
  removeBtn: {
    minHeight: 44,
    justifyContent: "center",
    paddingHorizontal: spacing.sm,
  },
  removeText: { color: colors.danger, fontSize: 14 },
  center: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    paddingBottom: spacing.xl,
    gap: spacing.sm,
  },
  centerSub: { color: colors.textMuted, fontSize: 14, textAlign: "center" },
  emptyTitle: { color: colors.text, fontSize: 16, fontWeight: "600" },
  primaryBtn: {
    marginTop: spacing.sm,
    minHeight: 48,
    justifyContent: "center",
    alignItems: "center",
    paddingHorizontal: spacing.xl,
    borderRadius: radius.md,
    backgroundColor: colors.accent,
  },
  primaryBtnText: { color: "#0b0e14", fontSize: 16, fontWeight: "700" },
  secondaryBtn: {
    marginTop: spacing.md,
    minHeight: 48,
    justifyContent: "center",
    alignItems: "center",
    borderRadius: radius.md,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    backgroundColor: colors.surfaceAlt,
  },
  secondaryBtnText: { color: colors.text, fontSize: 15, fontWeight: "600" },
  pressed: { opacity: 0.6 },
});
