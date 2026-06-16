// The chat composer (Task 512 + Task 515). A multiline text input + send button,
// with an on-composer mic button that dictates speech into the input (Task 515).
//
// Mic states rendered: idle / listening / error. While listening the live
// partial transcript previews above the input; the final transcript fills the
// composer. The recognizer is the injectable `SpeechRecognizer` seam — tests
// pass `createMockRecognizer(...)`; the app passes the real (Tier-3) one. When no
// recognizer is available the mic button is hidden.
import { useCallback, useRef, useState } from "react";
import { Pressable, StyleSheet, Text, TextInput, View } from "react-native";

import { colors, radius, spacing } from "../theme/tokens";
import type { SpeechRecognizer } from "../voice/speech-recognizer";

export interface ComposerProps {
  /** Send the composed text. The parent clears the input via the controlled value. */
  onSend: (text: string) => void;
  /** Disable send + mic (e.g. while no Core is paired). */
  disabled?: boolean;
  /** Optional voice dictation seam (Task 515). When absent the mic is hidden. */
  recognizer?: SpeechRecognizer;
}

type MicState = "idle" | "listening" | "error";

export function Composer({ onSend, disabled, recognizer }: ComposerProps) {
  const [text, setText] = useState("");
  const [micState, setMicState] = useState<MicState>("idle");
  const [partial, setPartial] = useState("");
  const [micError, setMicError] = useState<string | null>(null);
  // The text already in the input when dictation began, so partials APPEND to it
  // rather than replacing what the user typed.
  const baseTextRef = useRef("");

  const trimmed = text.trim();
  const canSend = !disabled && trimmed.length > 0;
  const micAvailable = !!recognizer && recognizer.isAvailable();

  const send = useCallback(() => {
    if (!canSend) return;
    onSend(trimmed);
    setText("");
  }, [canSend, onSend, trimmed]);

  const startDictation = useCallback(async () => {
    if (!recognizer) return;
    setMicError(null);
    const status = await recognizer.requestPermission();
    if (status !== "granted") {
      setMicState("error");
      setMicError(
        status === "denied"
          ? "Microphone access denied. Enable it in Settings to dictate."
          : "Microphone permission is required to dictate.",
      );
      return;
    }
    baseTextRef.current = text;
    setPartial("");
    setMicState("listening");
    await recognizer.start({
      onPartial: (t) => setPartial(t),
      onFinal: (t) => {
        const base = baseTextRef.current;
        const joined = base ? `${base.replace(/\s*$/, "")} ${t}`.trim() : t;
        setText(joined);
        setPartial("");
        setMicState("idle");
      },
      onError: (message) => {
        setPartial("");
        setMicError(message);
        setMicState("error");
      },
    });
  }, [recognizer, text]);

  const stopDictation = useCallback(async () => {
    if (!recognizer) return;
    await recognizer.stop();
    // onFinal (fired by stop) resets state to idle; guard in case it didn't.
    setMicState((s) => (s === "listening" ? "idle" : s));
  }, [recognizer]);

  const toggleMic = useCallback(() => {
    if (micState === "listening") {
      void stopDictation();
    } else {
      void startDictation();
    }
  }, [micState, startDictation, stopDictation]);

  return (
    <View style={styles.wrap} testID="composer">
      {micState === "listening" ? (
        <View style={styles.partialBar} testID="composer-partial">
          <View style={styles.listeningDot} />
          <Text style={styles.partialText} numberOfLines={2}>
            {partial || "Listening…"}
          </Text>
        </View>
      ) : null}
      {micState === "error" && micError ? (
        <Text style={styles.micError} testID="composer-mic-error">
          {micError}
        </Text>
      ) : null}

      <View style={styles.inputRow}>
        <TextInput
          testID="composer-input"
          style={styles.input}
          value={text}
          onChangeText={setText}
          placeholder="Message Concerto…"
          placeholderTextColor={colors.textMuted}
          editable={!disabled}
          multiline
          accessibilityLabel="Message Concerto"
          submitBehavior="newline"
        />

        {micAvailable ? (
          <Pressable
            testID="composer-mic"
            onPress={toggleMic}
            disabled={disabled}
            accessibilityRole="button"
            accessibilityState={{ disabled: !!disabled, selected: micState === "listening" }}
            accessibilityLabel={micState === "listening" ? "Stop dictation" : "Dictate message"}
            style={({ pressed }) => [
              styles.iconBtn,
              micState === "listening" && styles.micActive,
              pressed && styles.pressed,
              disabled && styles.btnDisabled,
            ]}
          >
            <Text style={styles.micGlyph} accessibilityElementsHidden importantForAccessibility="no">
              {micState === "listening" ? "■" : "🎤"}
            </Text>
          </Pressable>
        ) : null}

        <Pressable
          testID="composer-send"
          onPress={send}
          disabled={!canSend}
          accessibilityRole="button"
          accessibilityState={{ disabled: !canSend }}
          accessibilityLabel="Send message"
          style={({ pressed }) => [
            styles.sendBtn,
            !canSend && styles.btnDisabled,
            pressed && canSend && styles.pressed,
          ]}
        >
          <Text style={styles.sendGlyph} accessibilityElementsHidden importantForAccessibility="no">
            ↑
          </Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: {
    borderTopColor: colors.border,
    borderTopWidth: StyleSheet.hairlineWidth,
    backgroundColor: colors.surface,
    paddingHorizontal: spacing.md,
    paddingTop: spacing.sm,
    paddingBottom: spacing.md,
    gap: spacing.sm,
  },
  partialBar: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    backgroundColor: colors.surfaceAlt,
    borderRadius: radius.sm,
    paddingHorizontal: spacing.md,
    paddingVertical: spacing.sm,
  },
  listeningDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    backgroundColor: colors.danger,
  },
  partialText: {
    flex: 1,
    color: colors.text,
    fontSize: 14,
    fontStyle: "italic",
  },
  micError: {
    color: colors.danger,
    fontSize: 13,
    paddingHorizontal: spacing.xs,
  },
  inputRow: {
    flexDirection: "row",
    alignItems: "flex-end",
    gap: spacing.sm,
  },
  input: {
    flex: 1,
    minHeight: 44,
    maxHeight: 120,
    color: colors.text,
    fontSize: 16,
    backgroundColor: colors.surfaceAlt,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: radius.md,
    paddingHorizontal: spacing.md,
    paddingTop: spacing.sm,
    paddingBottom: spacing.sm,
  },
  iconBtn: {
    width: 44,
    height: 44,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: radius.md,
    backgroundColor: colors.surfaceAlt,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
  },
  micActive: {
    backgroundColor: colors.danger,
    borderColor: colors.danger,
  },
  micGlyph: {
    fontSize: 18,
    color: colors.text,
  },
  sendBtn: {
    width: 44,
    height: 44,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: radius.md,
    backgroundColor: colors.accent,
  },
  sendGlyph: {
    fontSize: 22,
    fontWeight: "800",
    color: "#0b0e14",
  },
  btnDisabled: {
    opacity: 0.4,
  },
  pressed: {
    opacity: 0.6,
  },
});
