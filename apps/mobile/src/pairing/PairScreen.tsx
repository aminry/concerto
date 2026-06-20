// The Pair-a-Core screen (Task 511; design/16 §3.8). An `expo-camera` QR scanner
// with a confirm → handshake → persist flow, plus a manual-code fallback (the
// design's "Enter pairing code manually" path for bad lighting). Mirrors the
// existing screens' UX: dark tokens, a11y labels + roles, >= 44pt targets, safe
// areas, and explicit loading / scanning / confirming / error / done states.
//
// The real camera + native pairing module are Tier-3; the screen is fully
// Tier-2-testable because (a) expo-camera is mocked in jest and (b) the pairing
// side effect is the injectable `onPair` prop (defaults to `pairWithQr` over the
// native module, resolved lazily so importing this file never needs the binding).
import { useCallback, useMemo, useState } from "react";
import {
  ActivityIndicator,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { CameraView, useCameraPermissions } from "expo-camera";

import { colors, radius, spacing } from "../theme/tokens";
import { ConnectBlobParseError, parseConnectBlob } from "./connect-blob";
import { pairWithQr, type PairResult } from "./pair";
import { getNativeConcertoIroh } from "../native/ConcertoIroh";

export interface PairScreenProps {
  /**
   * Runs the pairing handshake for a scanned/pasted QR string and persists the
   * Core. Defaults to `pairWithQr` over the REAL native module (Tier-3); tests
   * inject a stub. Resolving with the new Core completes the flow.
   */
  onPair?: (qr: string) => Promise<PairResult>;
  /** Called when pairing succeeds (the route navigates away / refreshes). */
  onPaired?: (result: PairResult) => void;
  /** Called when the user backs out without pairing. */
  onCancel?: () => void;
}

type Phase =
  | { kind: "scanning" }
  | { kind: "pairing" }
  | { kind: "error"; message: string }
  | { kind: "done"; label: string };

const defaultOnPair = (qr: string): Promise<PairResult> =>
  pairWithQr(getNativeConcertoIroh(), qr);

export function PairScreen({ onPair, onPaired, onCancel }: PairScreenProps) {
  const pair = useMemo(() => onPair ?? defaultOnPair, [onPair]);
  const [permission, requestPermission] = useCameraPermissions();
  const [phase, setPhase] = useState<Phase>({ kind: "scanning" });
  const [manual, setManual] = useState("");
  const [showManual, setShowManual] = useState(false);

  const handlePayload = useCallback(
    async (raw: string) => {
      // Validate locally first so an obviously-bad QR never spins the handshake.
      try {
        parseConnectBlob(raw);
      } catch (err) {
        setPhase({
          kind: "error",
          message:
            err instanceof ConnectBlobParseError
              ? err.message
              : "That QR code isn't a Concerto pairing code.",
        });
        return;
      }
      setPhase({ kind: "pairing" });
      try {
        const result = await pair(raw);
        setPhase({ kind: "done", label: result.core.label });
        onPaired?.(result);
      } catch (err) {
        setPhase({
          kind: "error",
          message: err instanceof Error ? err.message : "Pairing failed. Try again.",
        });
      }
    },
    [pair, onPaired],
  );

  const onScan = useCallback(
    (result: { data: string }) => {
      // Ignore repeat scans once we've left the scanning phase.
      if (phase.kind !== "scanning") return;
      void handlePayload(result.data);
    },
    [phase.kind, handlePayload],
  );

  const reset = useCallback(() => {
    setPhase({ kind: "scanning" });
    setManual("");
  }, []);

  return (
    <SafeAreaView style={styles.screen} edges={["top", "left", "right"]} testID="pair-screen">
      <View style={styles.header}>
        <Pressable
          testID="pair-cancel"
          onPress={onCancel}
          accessibilityRole="button"
          accessibilityLabel="Cancel pairing"
          style={({ pressed }) => [styles.backBtn, pressed && styles.pressed]}
        >
          <Text style={styles.backText}>‹ Back</Text>
        </Pressable>
        <Text style={styles.title} accessibilityRole="header">
          Pair a Core
        </Text>
        <View style={styles.backBtn} />
      </View>

      {phase.kind === "done" ? (
        <View style={styles.center} testID="pair-done">
          <Text style={styles.doneIcon} accessibilityElementsHidden importantForAccessibility="no">
            ✓
          </Text>
          <Text style={styles.doneTitle}>Paired with {phase.label}</Text>
          <Text style={styles.centerSub}>You can now use this Core from your phone.</Text>
        </View>
      ) : phase.kind === "pairing" ? (
        <View style={styles.center} testID="pair-pairing">
          <ActivityIndicator color={colors.accent} />
          <Text style={styles.centerSub}>Pairing with your Core…</Text>
        </View>
      ) : phase.kind === "error" ? (
        <View style={styles.center} testID="pair-error">
          <Text style={styles.errorTitle}>Couldn&rsquo;t pair</Text>
          <Text style={styles.centerSub}>{phase.message}</Text>
          <Pressable
            testID="pair-retry"
            onPress={reset}
            accessibilityRole="button"
            accessibilityLabel="Try pairing again"
            style={({ pressed }) => [styles.primaryBtn, pressed && styles.pressed]}
          >
            <Text style={styles.primaryBtnText}>Try again</Text>
          </Pressable>
        </View>
      ) : !permission ? (
        <View style={styles.center} testID="pair-permission-loading">
          <ActivityIndicator color={colors.accent} />
        </View>
      ) : !permission.granted ? (
        <View style={styles.center} testID="pair-permission-denied">
          <Text style={styles.emptyTitle}>Camera access needed</Text>
          <Text style={styles.centerSub}>
            Concerto uses the camera to scan the pairing QR code from your Core.
          </Text>
          <Pressable
            testID="pair-grant"
            onPress={requestPermission}
            accessibilityRole="button"
            accessibilityLabel="Allow camera access"
            style={({ pressed }) => [styles.primaryBtn, pressed && styles.pressed]}
          >
            <Text style={styles.primaryBtnText}>Allow camera</Text>
          </Pressable>
        </View>
      ) : (
        <View style={styles.scanArea} testID="pair-scanning">
          <CameraView
            testID="pair-camera"
            style={styles.camera}
            facing="back"
            barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
            onBarcodeScanned={onScan}
          />
          <View style={styles.reticle} pointerEvents="none" />
          <Text style={styles.hint}>
            Point your camera at the pairing QR code in your Core&rsquo;s tray.
          </Text>

          {showManual ? (
            <View style={styles.manualBox} testID="pair-manual">
              <TextInput
                testID="pair-manual-input"
                value={manual}
                onChangeText={setManual}
                placeholder="Paste pairing code"
                placeholderTextColor={colors.textMuted}
                autoCapitalize="none"
                autoCorrect={false}
                multiline
                accessibilityLabel="Pairing code"
                style={styles.manualInput}
              />
              <Pressable
                testID="pair-manual-submit"
                onPress={() => void handlePayload(manual)}
                accessibilityRole="button"
                accessibilityLabel="Pair with the entered code"
                style={({ pressed }) => [styles.primaryBtn, pressed && styles.pressed]}
              >
                <Text style={styles.primaryBtnText}>Pair</Text>
              </Pressable>
            </View>
          ) : (
            <Pressable
              testID="pair-manual-toggle"
              onPress={() => setShowManual(true)}
              accessibilityRole="button"
              accessibilityLabel="Enter pairing code manually"
              style={({ pressed }) => [styles.linkBtn, pressed && styles.pressed]}
            >
              <Text style={styles.linkText}>Enter code manually</Text>
            </Pressable>
          )}
        </View>
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
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingVertical: spacing.md,
  },
  backBtn: {
    minWidth: 64,
    minHeight: 44,
    justifyContent: "center",
  },
  backText: {
    color: colors.accent,
    fontSize: 16,
  },
  title: {
    color: colors.text,
    fontSize: 18,
    fontWeight: "700",
  },
  scanArea: {
    flex: 1,
    alignItems: "center",
    gap: spacing.md,
    paddingTop: spacing.md,
  },
  camera: {
    width: "100%",
    aspectRatio: 1,
    borderRadius: radius.lg,
    overflow: "hidden",
    backgroundColor: colors.surface,
  },
  reticle: {
    position: "absolute",
    top: spacing.md,
    width: "70%",
    aspectRatio: 1,
    borderColor: colors.accent,
    borderWidth: 2,
    borderRadius: radius.lg,
  },
  hint: {
    color: colors.textMuted,
    fontSize: 14,
    textAlign: "center",
    paddingHorizontal: spacing.lg,
  },
  manualBox: {
    width: "100%",
    gap: spacing.sm,
  },
  manualInput: {
    minHeight: 64,
    color: colors.text,
    backgroundColor: colors.surface,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: radius.md,
    padding: spacing.md,
    fontSize: 14,
  },
  center: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    paddingBottom: spacing.xl,
    gap: spacing.sm,
  },
  centerSub: {
    color: colors.textMuted,
    fontSize: 14,
    textAlign: "center",
    paddingHorizontal: spacing.lg,
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
  doneIcon: {
    color: colors.success,
    fontSize: 44,
    fontWeight: "700",
  },
  doneTitle: {
    color: colors.text,
    fontSize: 18,
    fontWeight: "700",
  },
  primaryBtn: {
    marginTop: spacing.sm,
    minHeight: 48,
    justifyContent: "center",
    alignItems: "center",
    paddingHorizontal: spacing.xl,
    borderRadius: radius.md,
    backgroundColor: colors.accent,
  },
  primaryBtnText: {
    color: "#0b0e14",
    fontSize: 16,
    fontWeight: "700",
  },
  linkBtn: {
    minHeight: 44,
    justifyContent: "center",
  },
  linkText: {
    color: colors.accent,
    fontSize: 15,
  },
  pressed: {
    opacity: 0.6,
  },
});
