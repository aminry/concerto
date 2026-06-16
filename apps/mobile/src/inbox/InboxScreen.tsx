// The Inbox screen (Task 508) — a fresh React Native component tree, NOT a port
// of the web/desktop @concerto/ui renderer (PHASE5_PLANNING D11). It renders the
// chronological notification feed wired to @concerto/client's generated
// `Notification` type. The live transport (the native ConcertoIroh DataClient)
// lands in Task 510; for now this is the app-shell stub the bottom-tab nav
// (Task 512) mounts, and it accepts an optional `items` prop so tests render a
// deterministic feed without a live Core.
import { FlatList, StyleSheet, Text, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";

import { colors, radius, spacing, severityColor } from "../theme/tokens";
import { kindLabel, relativeTime } from "./kind-label";

export interface InboxScreenProps {
  /**
   * The notifications to render. Defaults to an empty feed (the pre-pairing /
   * pre-live-transport state). Task 510 wires this to a live `DataClient`.
   */
  items?: Notification[];
}

function NotificationCard({ item }: { item: Notification }) {
  return (
    <View style={styles.card} testID="notification">
      <View style={[styles.accent, { backgroundColor: severityColor(item.severity || "low") }]} />
      <View style={styles.cardBody}>
        <View style={styles.cardHead}>
          <Text style={styles.kind}>{kindLabel(item.kind)}</Text>
          <Text style={styles.dot}>·</Text>
          <Text style={[styles.sevTag, { color: severityColor(item.severity || "low") }]}>
            {item.severity || "low"}
          </Text>
          <View style={styles.spacer} />
          <Text style={styles.time}>{relativeTime(item.createdAtMs)}</Text>
        </View>
        <Text style={styles.cardTitle}>{item.title}</Text>
        {item.body ? <Text style={styles.cardText}>{item.body}</Text> : null}
      </View>
    </View>
  );
}

export function InboxScreen({ items = [] }: InboxScreenProps) {
  return (
    <SafeAreaView style={styles.screen} edges={["top", "left", "right"]} testID="inbox-screen">
      <Text style={styles.title}>Notifications</Text>
      {items.length === 0 ? (
        <View style={styles.empty} testID="inbox-empty">
          <Text style={styles.emptyTitle}>You&rsquo;re all caught up</Text>
          <Text style={styles.emptySub}>Notifications from your Core will show up here.</Text>
        </View>
      ) : (
        <FlatList
          testID="inbox-feed"
          data={items}
          keyExtractor={(n) => n.id}
          renderItem={({ item }) => <NotificationCard item={item} />}
          contentContainerStyle={styles.feed}
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
  feed: {
    gap: spacing.sm,
    paddingBottom: spacing.xl,
  },
  empty: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    paddingBottom: spacing.xl,
  },
  emptyTitle: {
    color: colors.text,
    fontSize: 16,
    fontWeight: "600",
    marginBottom: spacing.xs,
  },
  emptySub: {
    color: colors.textMuted,
    fontSize: 13,
    textAlign: "center",
  },
  card: {
    flexDirection: "row",
    backgroundColor: colors.surface,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: radius.md,
    overflow: "hidden",
  },
  accent: {
    width: 4,
  },
  cardBody: {
    flex: 1,
    padding: spacing.md,
  },
  cardHead: {
    flexDirection: "row",
    alignItems: "center",
    marginBottom: spacing.xs,
  },
  kind: {
    color: colors.textMuted,
    fontSize: 12,
    fontWeight: "600",
  },
  dot: {
    color: colors.textMuted,
    marginHorizontal: spacing.xs,
  },
  sevTag: {
    fontSize: 12,
    fontWeight: "600",
    textTransform: "capitalize",
  },
  spacer: {
    flex: 1,
  },
  time: {
    color: colors.textMuted,
    fontSize: 12,
  },
  cardTitle: {
    color: colors.text,
    fontSize: 15,
    fontWeight: "600",
  },
  cardText: {
    color: colors.textMuted,
    fontSize: 13,
    marginTop: spacing.xs,
  },
});
