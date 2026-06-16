// The Concerto chat screen (Task 512) — the default landing tab (D14: "Concerto"
// is the user-facing name for the Maestro chat). A fresh RN component tree (NOT a
// port of @concerto/ui, D11) wired to the [`ChatClient`] seam (real generated
// `MaestroTurn` types via a mock client in tests / the app shell).
//
// Behaviour:
//   - seeds the transcript from `client.history()` (loading / error+retry states),
//   - user/assistant bubbles with timestamps; the in-flight assistant reply
//     streams token-by-token live into its bubble,
//   - composer (multiline + send + mic dictation) is keyboard-avoiding,
//   - auto-scrolls to the newest message,
//   - when NO Core is paired the whole surface is an empty state with a Pair CTA
//     (reusing the `PairEntry` affordance),
//   - a failed send marks that user bubble with an inline retry.
//
// Accessible + modern: a11y labels/roles, >=44pt touch targets, dark-first
// tokens, SafeAreaView, KeyboardAvoidingView.
import { useCallback, useEffect, useRef, useState } from "react";
import {
  ActivityIndicator,
  FlatList,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import { colors, radius, spacing } from "../theme/tokens";
import { PairEntry } from "../pairing/PairEntry";
import type { SpeechRecognizer } from "../voice/speech-recognizer";
import { Composer } from "./Composer";
import type { ChatClient, ChatTurn } from "./chat-client";

export interface ChatScreenProps {
  /** The chat data seam. Tests pass a `mockChatClient(...)`; the app passes the live one. */
  client: ChatClient;
  /**
   * Whether a Core is paired. When false the screen shows the unpaired empty
   * state with the Pair CTA instead of the transcript/composer.
   */
  hasCore?: boolean;
  /** Open the pairing flow (the route file wires this to `router.push("/pair")`). */
  onPair?: () => void;
  /** Open the multi-Core picker (the route file wires this to `router.push("/cores")`). */
  onManageCores?: () => void;
  /** Optional voice dictation seam (Task 515), forwarded to the composer. */
  recognizer?: SpeechRecognizer;
}

/** A rendered message: a history/sent turn plus transient streaming + send state. */
interface ChatMessage {
  /** Stable key. */
  id: string;
  role: "user" | "assistant";
  text: string;
  createdAtMs: bigint;
  /** Assistant reply still receiving tokens (shows a typing caret). */
  streaming?: boolean;
  /** User message whose send failed (shows inline retry). */
  failed?: boolean;
}

type LoadState =
  | { phase: "loading" }
  | { phase: "error"; message: string }
  | { phase: "ready" };

function fmtTime(ms: bigint): string {
  const d = new Date(Number(ms));
  return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

function turnToMessage(turn: ChatTurn, idx: number): ChatMessage {
  return {
    id: `h-${idx}-${turn.createdAtMs.toString()}`,
    role: turn.role === "assistant" ? "assistant" : "user",
    text: turn.text,
    createdAtMs: turn.createdAtMs,
  };
}

function MessageBubble({
  message,
  onRetry,
}: {
  message: ChatMessage;
  onRetry?: () => void;
}) {
  const mine = message.role === "user";
  return (
    <View
      testID={`chat-message-${message.id}`}
      style={[styles.bubbleRow, mine ? styles.bubbleRowMine : styles.bubbleRowTheirs]}
    >
      <View
        accessibilityRole="text"
        accessibilityLabel={`${mine ? "You" : "Concerto"}: ${message.text}`}
        style={[styles.bubble, mine ? styles.bubbleMine : styles.bubbleTheirs]}
      >
        <Text style={[styles.bubbleText, mine ? styles.bubbleTextMine : styles.bubbleTextTheirs]}>
          {message.text}
          {message.streaming ? <Text style={styles.caret}>▍</Text> : null}
        </Text>
        <View style={styles.metaRow}>
          <Text style={[styles.time, mine ? styles.timeMine : styles.timeTheirs]}>
            {fmtTime(message.createdAtMs)}
          </Text>
          {message.failed ? (
            <Pressable
              testID={`chat-retry-${message.id}`}
              onPress={onRetry}
              accessibilityRole="button"
              accessibilityLabel="Retry sending this message"
              hitSlop={8}
              style={({ pressed }) => [styles.retryInline, pressed && styles.pressed]}
            >
              <Text style={styles.retryInlineText}>Failed · Retry</Text>
            </Pressable>
          ) : null}
        </View>
      </View>
    </View>
  );
}

export function ChatScreen({
  client,
  hasCore = true,
  onPair,
  onManageCores,
  recognizer,
}: ChatScreenProps) {
  const [load, setLoad] = useState<LoadState>({ phase: "loading" });
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const listRef = useRef<FlatList<ChatMessage>>(null);
  const seq = useRef(0);
  const nextId = (p: string) => `${p}-${(seq.current += 1)}`;

  const loadHistory = useCallback(() => {
    let cancelled = false;
    setLoad({ phase: "loading" });
    client
      .history()
      .then((turns) => {
        if (cancelled) return;
        setMessages(turns.map(turnToMessage));
        setLoad({ phase: "ready" });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setLoad({
          phase: "error",
          message: err instanceof Error ? err.message : "Couldn't load the conversation.",
        });
      });
    return () => {
      cancelled = true;
    };
  }, [client]);

  // Only load history once a Core is paired (unpaired => the Pair CTA, no RPC).
  useEffect(() => {
    if (!hasCore) {
      setLoad({ phase: "ready" });
      return;
    }
    return loadHistory();
  }, [hasCore, loadHistory]);

  const scrollToEnd = useCallback(() => {
    requestAnimationFrame(() => listRef.current?.scrollToEnd({ animated: true }));
  }, []);

  // Stream an assistant reply into a fresh assistant bubble.
  const streamReply = useCallback(
    async (assistantId: string, stream: AsyncIterable<string>) => {
      try {
        for await (const chunk of stream) {
          setMessages((prev) =>
            prev.map((m) => (m.id === assistantId ? { ...m, text: m.text + chunk } : m)),
          );
          scrollToEnd();
        }
      } catch {
        // Stream error: surface a short note in the assistant bubble.
        setMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId
              ? { ...m, text: m.text || "Concerto couldn't finish replying.", streaming: false }
              : m,
          ),
        );
        return;
      }
      setMessages((prev) =>
        prev.map((m) => (m.id === assistantId ? { ...m, streaming: false } : m)),
      );
    },
    [scrollToEnd],
  );

  const send = useCallback(
    (text: string, existingId?: string) => {
      const userId = existingId ?? nextId("u");
      setMessages((prev) => {
        if (existingId) {
          return prev.map((m) => (m.id === existingId ? { ...m, failed: false } : m));
        }
        return [
          ...prev,
          { id: userId, role: "user", text, createdAtMs: BigInt(Date.now()) },
        ];
      });
      scrollToEnd();

      client
        .send(text)
        .then((stream) => {
          const assistantId = nextId("a");
          setMessages((prev) => [
            ...prev,
            {
              id: assistantId,
              role: "assistant",
              text: "",
              createdAtMs: BigInt(Date.now()),
              streaming: true,
            },
          ]);
          scrollToEnd();
          void streamReply(assistantId, stream.tokens);
        })
        .catch(() => {
          setMessages((prev) =>
            prev.map((m) => (m.id === userId ? { ...m, failed: true } : m)),
          );
        });
    },
    [client, scrollToEnd, streamReply],
  );

  // ── Unpaired: the whole surface is the Pair CTA empty state ────────────────
  if (!hasCore) {
    return (
      <SafeAreaView style={styles.screen} edges={["top", "left", "right"]} testID="chat-screen">
        <Text style={styles.title} accessibilityRole="header">
          Concerto
        </Text>
        <View style={styles.center} testID="chat-empty-unpaired">
          <Text style={styles.emptyTitle}>Pair a Core to start chatting</Text>
          <Text style={styles.centerSub}>
            Concerto runs on your Core. Pair one to talk to it from here.
          </Text>
          <PairEntry onPair={() => onPair?.()} {...(onManageCores ? { onManageCores } : {})} />
        </View>
      </SafeAreaView>
    );
  }

  return (
    <SafeAreaView style={styles.screen} edges={["top", "left", "right"]} testID="chat-screen">
      <Text style={styles.title} accessibilityRole="header">
        Concerto
      </Text>

      <KeyboardAvoidingView
        style={styles.flex}
        behavior={Platform.OS === "ios" ? "padding" : undefined}
        keyboardVerticalOffset={Platform.OS === "ios" ? 8 : 0}
      >
        {load.phase === "loading" ? (
          <View style={styles.center} testID="chat-loading">
            <ActivityIndicator color={colors.accent} />
            <Text style={styles.centerSub}>Loading conversation…</Text>
          </View>
        ) : load.phase === "error" ? (
          <View style={styles.center} testID="chat-error">
            <Text style={styles.errorTitle}>Couldn&rsquo;t load the conversation</Text>
            <Text style={styles.centerSub}>{load.message}</Text>
            <Pressable
              testID="chat-retry"
              onPress={loadHistory}
              accessibilityRole="button"
              accessibilityLabel="Retry loading the conversation"
              style={({ pressed }) => [styles.retry, pressed && styles.pressed]}
            >
              <Text style={styles.retryText}>Try again</Text>
            </Pressable>
          </View>
        ) : messages.length === 0 ? (
          <View style={styles.center} testID="chat-empty">
            <Text style={styles.emptyTitle}>Say hello to Concerto</Text>
            <Text style={styles.centerSub}>
              Ask about your workspaces, kick off work, or just chat. Replies stream in live.
            </Text>
          </View>
        ) : (
          <FlatList
            ref={listRef}
            testID="chat-list"
            data={messages}
            keyExtractor={(m) => m.id}
            renderItem={({ item }) => (
              <MessageBubble
                message={item}
                onRetry={item.failed ? () => send(item.text, item.id) : undefined}
              />
            )}
            contentContainerStyle={styles.list}
            onContentSizeChange={scrollToEnd}
          />
        )}

        <Composer onSend={send} {...(recognizer ? { recognizer } : {})} />
      </KeyboardAvoidingView>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  screen: {
    flex: 1,
    backgroundColor: colors.bg,
  },
  title: {
    color: colors.text,
    fontSize: 22,
    fontWeight: "700",
    paddingHorizontal: spacing.lg,
    paddingTop: spacing.lg,
    paddingBottom: spacing.sm,
  },
  list: {
    paddingHorizontal: spacing.lg,
    paddingBottom: spacing.md,
    gap: spacing.sm,
  },
  bubbleRow: {
    flexDirection: "row",
  },
  bubbleRowMine: {
    justifyContent: "flex-end",
  },
  bubbleRowTheirs: {
    justifyContent: "flex-start",
  },
  bubble: {
    maxWidth: "84%",
    borderRadius: radius.lg,
    paddingHorizontal: spacing.md,
    paddingVertical: spacing.sm,
  },
  bubbleMine: {
    backgroundColor: colors.accent,
    borderTopRightRadius: radius.sm,
  },
  bubbleTheirs: {
    backgroundColor: colors.surface,
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderTopLeftRadius: radius.sm,
  },
  bubbleText: {
    fontSize: 15,
    lineHeight: 20,
  },
  bubbleTextMine: { color: "#0b0e14" },
  bubbleTextTheirs: { color: colors.text },
  caret: {
    color: colors.accent,
  },
  metaRow: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "flex-end",
    gap: spacing.sm,
    marginTop: spacing.xs / 2,
  },
  time: {
    fontSize: 11,
  },
  timeMine: { color: "rgba(11,14,20,0.6)" },
  timeTheirs: { color: colors.textMuted },
  retryInline: {
    minHeight: 24,
    justifyContent: "center",
  },
  retryInlineText: {
    color: colors.danger,
    fontSize: 12,
    fontWeight: "700",
  },
  center: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    paddingHorizontal: spacing.xl,
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
  pressed: {
    opacity: 0.6,
  },
});
