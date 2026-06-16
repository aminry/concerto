// Pure-RN, touch-first unified-diff renderer (Task 514). Renders the flat
// `DiffRow[]` from `parseUnifiedDiff` as a VIRTUALIZED FlatList so a large diff
// (the spike-103 1000-line budget) stays scrollable without mounting every row.
//
// Design choices (all Tier-2 / no extra native deps):
//   * VIRTUALIZED — one FlatList over rows; `removeClippedSubviews` + a measured
//     `getItemLayout` (every row is a fixed `ROW_HEIGHT`) so scroll stays O(1).
//   * COLLAPSIBLE HUNKS — tapping a hunk header toggles a JS `Set<hunkId>`; we
//     recompute the visible row list (cheap, memoized) instead of using any
//     native pager/gesture-handler. Pure `Pressable` taps, >=44pt targets.
//   * LONG LINES — each body row's content sits in its OWN horizontal ScrollView
//     so a wide line scrolls sideways independently (GitHub-mobile style) without
//     wrapping or breaking the line-number gutter alignment.
//   * COLORS — strictly from `theme/tokens` (dark-first); add/remove washes +
//     colored gutters; the +/- marker is a dedicated column.
//
// PERF BUDGET (spike-103): a 1000-line diff should scroll at 60fps and first-
// paint < 1.5s on iPhone 13+ / Pixel 6+. That on-device measurement is a
// **Tier-3** verification line (see DiffView.PERF_BUDGET) — NOT gated here. This
// component ships behind the documented V1.5 native-diff fallback: if the RN
// renderer misses the budget on-device, the fallback path renders the diff
// natively. The fallback decision is GO/NO-GO at Tier-3, not a blocker for this
// task.
import { useCallback, useMemo, useState } from "react";
import {
  FlatList,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
  type ListRenderItemInfo,
} from "react-native";

import { colors, mono, spacing } from "../theme/tokens";
import {
  parseUnifiedDiff,
  summarizeRows,
  type DiffRow,
} from "./parse-unified-diff";

/**
 * The spike-103 performance budget for the RN diff renderer. The on-device
 * measurement (1000-line diff: first paint < 1.5s, scroll at 60fps on an
 * iPhone 13+ / Pixel 6+) is a **Tier-3** verification — it cannot be asserted in
 * the jest runtime. Exported so the demo screen + docs can surface it verbatim.
 */
export const PERF_BUDGET = {
  diffLines: 1000,
  firstPaintMs: 1500,
  targetFps: 60,
  devices: ["iPhone 13+", "Pixel 6+"],
  /** When the budget is missed on-device, fall back to the native diff (V1.5). */
  fallback: "V1.5 native-diff renderer",
} as const;

/** Fixed row height — single source of truth for `getItemLayout` virtualization. */
const ROW_HEIGHT = 20;
/** Width of each line-number gutter column. */
const GUTTER_WIDTH = 44;

export interface DiffViewProps {
  /** Raw unified-diff text (parsed internally) … */
  diffText?: string;
  /** … or pre-parsed rows (skip the parser; handy for very large fixtures). */
  rows?: DiffRow[];
  /** Hunks start collapsed when true (default: expanded). */
  initiallyCollapsed?: boolean;
  testID?: string;
}

/** A hunk header row is what the user taps to collapse/expand its body. */
function HunkHeader({
  row,
  collapsed,
  onToggle,
}: {
  row: Extract<DiffRow, { kind: "hunk" }>;
  collapsed: boolean;
  onToggle: (hunkId: string) => void;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityState={{ expanded: !collapsed }}
      accessibilityLabel={`${collapsed ? "Expand" : "Collapse"} hunk ${row.header}`}
      onPress={() => onToggle(row.hunkId)}
      // A taller hit slop gives the compact (ROW_HEIGHT) header row a >=44pt
      // touch target without growing the visual row (keeps code alignment).
      hitSlop={{ top: 12, bottom: 12, left: 0, right: 0 }}
      style={({ pressed }) => [
        styles.row,
        styles.hunkRow,
        pressed && styles.hunkPressed,
      ]}
      testID={`diff-hunk-toggle-${row.hunkId}`}
    >
      <Text style={styles.hunkChevron}>{collapsed ? "▸" : "▾"}</Text>
      <Text style={styles.hunkHeaderText} numberOfLines={1}>
        {" "}
        {row.header}
        {row.section ? ` ${row.section}` : ""}
      </Text>
    </Pressable>
  );
}

/** A file boundary header. */
function FileHeader({ row }: { row: Extract<DiffRow, { kind: "file" }> }) {
  return (
    <View style={[styles.row, styles.fileRow]} testID={`diff-file-${row.fileIndex}`}>
      <Text style={styles.fileText} numberOfLines={1} accessibilityRole="header">
        {row.oldPath && row.oldPath !== row.path
          ? `${row.oldPath} → ${row.path}`
          : row.path}
      </Text>
    </View>
  );
}

/** An add/remove/context body line: [old#][new#][±] [content (h-scroll)]. */
function BodyLine({ row }: { row: Exclude<DiffRow, { kind: "file" | "hunk" }> }) {
  const oldNum = row.kind === "add" ? "" : String(row.oldLine);
  const newNum = row.kind === "remove" ? "" : String(row.newLine);
  const marker = row.kind === "add" ? "+" : row.kind === "remove" ? "-" : " ";

  const rowStyle =
    row.kind === "add"
      ? styles.addRow
      : row.kind === "remove"
        ? styles.removeRow
        : null;
  const gutterStyle =
    row.kind === "add"
      ? styles.addGutter
      : row.kind === "remove"
        ? styles.removeGutter
        : styles.contextGutter;
  const markerColor =
    row.kind === "add"
      ? colors.diffAddMarker
      : row.kind === "remove"
        ? colors.diffRemoveMarker
        : colors.textMuted;

  // Accessibility: announce kind + line + content so VoiceOver reads a useful row.
  const a11yKind =
    row.kind === "add" ? "added" : row.kind === "remove" ? "removed" : "context";

  return (
    <View
      style={[styles.row, rowStyle]}
      testID={`diff-line-${row.key}`}
      accessibilityLabel={`${a11yKind} line ${newNum || oldNum}: ${row.content}`}
    >
      <View style={[styles.gutter, gutterStyle]}>
        <Text style={styles.gutterText} numberOfLines={1}>
          {oldNum}
        </Text>
      </View>
      <View style={[styles.gutter, gutterStyle]}>
        <Text style={styles.gutterText} numberOfLines={1}>
          {newNum}
        </Text>
      </View>
      <Text style={[styles.marker, { color: markerColor }]}>{marker}</Text>
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        style={styles.contentScroll}
        contentContainerStyle={styles.contentInner}
      >
        <Text style={styles.contentText}>{row.content === "" ? " " : row.content}</Text>
      </ScrollView>
    </View>
  );
}

export function DiffView({
  diffText,
  rows: rowsProp,
  initiallyCollapsed = false,
  testID = "diff-view",
}: DiffViewProps) {
  // Parse once per diffText change (or accept pre-parsed rows for big fixtures).
  const allRows = useMemo<DiffRow[]>(
    () => rowsProp ?? (diffText !== undefined ? parseUnifiedDiff(diffText) : []),
    [rowsProp, diffText],
  );

  const [collapsed, setCollapsed] = useState<Set<string>>(() => {
    if (!initiallyCollapsed) return new Set();
    const s = new Set<string>();
    for (const r of allRows) if (r.kind === "hunk") s.add(r.hunkId);
    return s;
  });

  const toggle = useCallback((hunkId: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(hunkId)) next.delete(hunkId);
      else next.add(hunkId);
      return next;
    });
  }, []);

  // The VISIBLE row list: drop body rows whose hunk is collapsed. File + hunk
  // header rows are always visible. Memoized so a scroll doesn't recompute it.
  const visibleRows = useMemo<DiffRow[]>(() => {
    if (collapsed.size === 0) return allRows;
    return allRows.filter((r) => {
      if (r.kind === "file" || r.kind === "hunk") return true;
      return !collapsed.has(r.hunkId);
    });
  }, [allRows, collapsed]);

  const summary = useMemo(() => summarizeRows(allRows), [allRows]);

  const renderItem = useCallback(
    ({ item }: ListRenderItemInfo<DiffRow>) => {
      if (item.kind === "file") return <FileHeader row={item} />;
      if (item.kind === "hunk") {
        return (
          <HunkHeader
            row={item}
            collapsed={collapsed.has(item.hunkId)}
            onToggle={toggle}
          />
        );
      }
      return <BodyLine row={item} />;
    },
    [collapsed, toggle],
  );

  // Every row is exactly ROW_HEIGHT tall -> O(1) scroll offset math.
  const getItemLayout = useCallback(
    (_data: ArrayLike<DiffRow> | null | undefined, index: number) => ({
      length: ROW_HEIGHT,
      offset: ROW_HEIGHT * index,
      index,
    }),
    [],
  );

  if (allRows.length === 0) {
    return (
      <View style={styles.empty} testID={`${testID}-empty`}>
        <Text style={styles.emptyText}>No changes to show.</Text>
      </View>
    );
  }

  return (
    <View style={styles.container} testID={testID}>
      <View style={styles.summaryBar} testID={`${testID}-summary`}>
        <Text style={styles.summaryText}>
          {summary.files} file{summary.files === 1 ? "" : "s"}
        </Text>
        <Text style={[styles.summaryText, styles.summaryAdd]}>+{summary.added}</Text>
        <Text style={[styles.summaryText, styles.summaryRemove]}>
          −{summary.removed}
        </Text>
      </View>
      <FlatList
        testID={`${testID}-list`}
        data={visibleRows}
        keyExtractor={(r) => r.key}
        renderItem={renderItem}
        getItemLayout={getItemLayout}
        removeClippedSubviews
        initialNumToRender={40}
        maxToRenderPerBatch={40}
        windowSize={11}
        showsVerticalScrollIndicator
      />
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: colors.bg,
  },
  summaryBar: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingHorizontal: spacing.md,
    paddingVertical: spacing.sm,
    borderBottomColor: colors.border,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  summaryText: {
    color: colors.textMuted,
    fontSize: 13,
    fontVariant: ["tabular-nums"],
  },
  summaryAdd: { color: colors.diffAddMarker, fontWeight: "600" },
  summaryRemove: { color: colors.diffRemoveMarker, fontWeight: "600" },
  row: {
    flexDirection: "row",
    alignItems: "center",
    height: ROW_HEIGHT,
  },
  fileRow: {
    backgroundColor: colors.surfaceAlt,
    paddingHorizontal: spacing.sm,
    borderTopColor: colors.border,
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  fileText: {
    ...mono,
    color: colors.text,
    fontSize: 12,
    fontWeight: "700",
  },
  hunkRow: {
    backgroundColor: colors.diffHunkBg,
    paddingHorizontal: spacing.sm,
  },
  hunkPressed: {
    opacity: 0.6,
  },
  hunkChevron: {
    ...mono,
    color: colors.diffHunkText,
    fontSize: 12,
  },
  hunkHeaderText: {
    ...mono,
    color: colors.diffHunkText,
    fontSize: 12,
  },
  addRow: { backgroundColor: colors.diffAddBg },
  removeRow: { backgroundColor: colors.diffRemoveBg },
  gutter: {
    width: GUTTER_WIDTH,
    height: ROW_HEIGHT,
    justifyContent: "center",
    alignItems: "flex-end",
    paddingRight: spacing.xs,
  },
  addGutter: { backgroundColor: colors.diffAddGutter },
  removeGutter: { backgroundColor: colors.diffRemoveGutter },
  contextGutter: { backgroundColor: colors.surface },
  gutterText: {
    ...mono,
    color: colors.textMuted,
    fontSize: 11,
    fontVariant: ["tabular-nums"],
  },
  marker: {
    ...mono,
    width: 14,
    textAlign: "center",
    fontSize: 12,
  },
  contentScroll: {
    flex: 1,
  },
  contentInner: {
    alignItems: "center",
    paddingRight: spacing.lg,
  },
  contentText: {
    ...mono,
    color: colors.text,
    fontSize: 12,
  },
  empty: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    padding: spacing.xl,
  },
  emptyText: {
    color: colors.textMuted,
    fontSize: 14,
  },
});
