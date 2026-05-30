// Subscribe to `session.io.<sid>` and feed every `SessionIoChunk`'s
// raw bytes to the supplied sink.
//
// The Rust forwarder lives until `concerto_unsubscribe` is called;
// this hook owns the lifecycle. The callback ref pattern keeps the
// stream alive even when the parent component re-renders.

import { useEffect, useRef } from "react";

import {
  onConcertoEvent,
  subscribe,
  unsubscribe,
} from "../api/client";
import {
  chunkToBytes,
  oneofVariant,
  type SessionIoChunkPayload,
  type StreamEvent,
} from "../api/sessions";

export function useSessionIO(
  sessionId: string | null | undefined,
  onChunk: (bytes: Uint8Array, stream: string) => void,
): void {
  const callbackRef = useRef(onChunk);
  callbackRef.current = onChunk;

  useEffect(() => {
    if (!sessionId) return;

    const subject = `session.io.${sessionId}`;
    let subscriptionId: string | null = null;
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    void (async () => {
      try {
        unlisten = await onConcertoEvent<StreamEvent>(subject, (event) => {
          // The `body` oneof variant serializes as PascalCase `SessionIo`
          // (prost serde default), not snake_case — accept both.
          const chunk = oneofVariant<SessionIoChunkPayload>(
            event.body,
            "SessionIo",
            "session_io",
          );
          if (!chunk) return;
          callbackRef.current(chunkToBytes(chunk.data), chunk.stream);
        });
        // StrictMode runs the cleanup synchronously before these awaits
        // resolve, so it can't see `unlisten` yet — unlisten here if we
        // were already torn down, otherwise the JS listener leaks.
        if (cancelled) {
          unlisten?.();
          return;
        }
        const id = await subscribe(subject);
        if (cancelled) {
          unlisten?.();
          await unsubscribe(id);
          return;
        }
        subscriptionId = id;
      } catch (e) {
        console.error("useSessionIO failed", subject, e);
      }
    })();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
      if (subscriptionId) {
        void unsubscribe(subscriptionId);
      }
    };
  }, [sessionId]);
}
