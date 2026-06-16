// Localhost preview tunnel screen (Task 517). Requests a public tunnel URL for a
// workarea's dev server through the `TunnelClient` seam, then renders it in a
// `react-native-webview` WebView. Two layered states:
//   1. TUNNEL request: loading / error (with retry) / ready (we have a URL).
//   2. WEBVIEW load:    a loading overlay until `onLoadEnd`, an error state on
//      `onError` / `onHttpError`, both inside the ready phase.
// Plus an "Open in browser" affordance (system browser via `Linking`).
//
// Tier-2: the WebView is jest-mocked (real page loads are Tier-3); the tunnel
// URL comes from a typed fixture/mock (no proto Tunnels service exists yet).
import { useCallback, useEffect, useState } from "react";
import {
  ActivityIndicator,
  Linking,
  Pressable,
  StyleSheet,
  Text,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { WebView } from "react-native-webview";

import { colors, radius, spacing } from "../theme/tokens";
import type { TunnelClient, TunnelInfo } from "./tunnel-client";

export interface PreviewScreenProps {
  client: TunnelClient;
  /** The workarea / dev-server id to tunnel (from `/preview/[id]`). */
  id: string;
  /** Back handler — the route wires this to `router.back()`. */
  onBack?: () => void;
  /** Injectable opener (defaults to `Linking.openURL`) — lets tests assert it. */
  openUrl?: (url: string) => Promise<unknown>;
}

type TunnelPhase =
  | { phase: "loading" }
  | { phase: "error"; message: string }
  | { phase: "ready"; tunnel: TunnelInfo };

type WebPhase = "loading" | "ready" | "error";

function errMessage(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback;
}

export function PreviewScreen({ client, id, onBack, openUrl }: PreviewScreenProps) {
  const [tunnel, setTunnel] = useState<TunnelPhase>({ phase: "loading" });
  const [web, setWeb] = useState<WebPhase>("loading");

  const load = useCallback(() => {
    let cancelled = false;
    setTunnel({ phase: "loading" });
    setWeb("loading");
    client
      .startLocalhostTunnel(id)
      .then((t) => !cancelled && setTunnel({ phase: "ready", tunnel: t }))
      .catch(
        (err) =>
          !cancelled &&
          setTunnel({ phase: "error", message: errMessage(err, "Couldn't start the preview tunnel.") }),
      );
    return () => {
      cancelled = true;
    };
  }, [client, id]);

  useEffect(() => load(), [load]);

  const url = tunnel.phase === "ready" ? tunnel.tunnel.url : null;

  const onOpenInBrowser = useCallback(() => {
    if (!url) return;
    const opener = openUrl ?? ((u: string) => Linking.openURL(u));
    void opener(url);
  }, [url, openUrl]);

  return (
    <SafeAreaView style={styles.screen} edges={["top", "left", "right"]} testID="preview-screen">
      <View style={styles.header}>
        <Pressable
          testID="preview-back"
          onPress={onBack}
          accessibilityRole="button"
          accessibilityLabel="Back"
          style={({ pressed }) => [styles.backBtn, pressed && styles.pressed]}
        >
          <Text style={styles.backText}>‹ Back</Text>
        </Pressable>
        <Text style={styles.title} numberOfLines={1} accessibilityRole="header">
          Preview
        </Text>
        <View style={styles.spacer} />
        <Pressable
          testID="preview-open-browser"
          onPress={onOpenInBrowser}
          disabled={!url}
          accessibilityRole="button"
          accessibilityLabel="Open preview in browser"
          accessibilityState={{ disabled: !url }}
          style={({ pressed }) => [
            styles.openBtn,
            !url && styles.openBtnDisabled,
            pressed && styles.pressed,
          ]}
        >
          <Text style={[styles.openText, !url && styles.openTextDisabled]}>Open in browser</Text>
        </Pressable>
      </View>

      {url ? (
        <Text style={styles.urlBar} numberOfLines={1} testID="preview-url">
          {url}
        </Text>
      ) : null}

      <View style={styles.body}>
        {tunnel.phase === "loading" ? (
          <View style={styles.center} testID="preview-tunnel-loading">
            <ActivityIndicator color={colors.accent} />
            <Text style={styles.muted}>Starting preview tunnel…</Text>
          </View>
        ) : tunnel.phase === "error" ? (
          <View style={styles.center} testID="preview-tunnel-error">
            <Text style={styles.errorText}>{tunnel.message}</Text>
            <Pressable
              testID="preview-retry"
              onPress={load}
              accessibilityRole="button"
              accessibilityLabel="Retry starting the preview tunnel"
              style={({ pressed }) => [styles.retry, pressed && styles.pressed]}
            >
              <Text style={styles.retryText}>Try again</Text>
            </Pressable>
          </View>
        ) : (
          <View style={styles.webWrap}>
            <WebView
              testID="preview-webview"
              source={{ uri: tunnel.tunnel.url }}
              onLoadStart={() => setWeb("loading")}
              onLoadEnd={() => setWeb((w) => (w === "error" ? w : "ready"))}
              onError={() => setWeb("error")}
              onHttpError={() => setWeb("error")}
              style={styles.webview}
            />
            {web === "loading" ? (
              <View style={styles.webOverlay} testID="preview-web-loading" pointerEvents="none">
                <ActivityIndicator color={colors.accent} />
              </View>
            ) : null}
            {web === "error" ? (
              <View style={styles.webOverlay} testID="preview-web-error">
                <Text style={styles.errorText}>The preview page failed to load.</Text>
                <Pressable
                  testID="preview-open-browser-fallback"
                  onPress={onOpenInBrowser}
                  accessibilityRole="button"
                  accessibilityLabel="Open preview in browser instead"
                  style={({ pressed }) => [styles.retry, pressed && styles.pressed]}
                >
                  <Text style={styles.retryText}>Open in browser</Text>
                </Pressable>
              </View>
            ) : null}
          </View>
        )}
      </View>
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
    paddingRight: spacing.xs,
  },
  backText: {
    color: colors.accent,
    fontSize: 16,
    fontWeight: "600",
  },
  pressed: { opacity: 0.6 },
  title: {
    color: colors.text,
    fontSize: 18,
    fontWeight: "700",
  },
  spacer: { flex: 1 },
  openBtn: {
    minHeight: 44,
    justifyContent: "center",
    paddingHorizontal: spacing.md,
    borderRadius: radius.sm,
    backgroundColor: colors.surfaceAlt,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
  },
  openBtnDisabled: { opacity: 0.4 },
  openText: {
    color: colors.text,
    fontSize: 13,
    fontWeight: "600",
  },
  openTextDisabled: { color: colors.textMuted },
  urlBar: {
    color: colors.textMuted,
    fontSize: 12,
    paddingHorizontal: spacing.lg,
    paddingTop: spacing.xs,
  },
  body: {
    flex: 1,
    marginTop: spacing.sm,
  },
  webWrap: {
    flex: 1,
  },
  webview: {
    flex: 1,
    backgroundColor: colors.bg,
  },
  webOverlay: {
    ...StyleSheet.absoluteFillObject,
    alignItems: "center",
    justifyContent: "center",
    gap: spacing.md,
    backgroundColor: colors.bg,
  },
  center: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    gap: spacing.md,
    padding: spacing.xl,
  },
  muted: {
    color: colors.textMuted,
    fontSize: 14,
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
