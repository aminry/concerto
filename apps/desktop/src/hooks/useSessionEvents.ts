// Subscribe to `session.events.<sid>` and surface a coarse `status`
// string that the session tab uses for its badge.
//
// V0.1 supports three `SessionEvent.kind` variants — `started`,
// `message`, `exited`. The hook collapses them into a status string
// matching the workarea status vocabulary (`starting | running |
// finished`). Phase 3 will add `awaiting`/`crashed` when the
// tool-approval frames land.

import { useEffect, useRef, useState } from "react";

import {
  onConcertoEvent,
  subscribe,
  unsubscribe,
} from "../api/client";
import {
  oneofVariant,
  type SessionEventPayload,
  type StreamEvent,
} from "../api/sessions";

export type SessionStatusBadge =
  | "starting"
  | "running"
  | "finished"
  | "crashed";

export type UseSessionEventsState = {
  status: SessionStatusBadge;
  exitCode: number | null;
};

export function useSessionEvents(
  sessionId: string | null | undefined,
  initialStatus: SessionStatusBadge = "starting",
): UseSessionEventsState {
  const [state, setState] = useState<UseSessionEventsState>({
    status: initialStatus,
    exitCode: null,
  });
  // Avoid resetting state when only initialStatus changes; the
  // subject change effect handles re-init explicitly.
  const initialRef = useRef(initialStatus);
  initialRef.current = initialStatus;

  useEffect(() => {
    if (!sessionId) return;

    const subject = `session.events.${sessionId}`;
    let subscriptionId: string | null = null;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    setState({ status: initialRef.current, exitCode: null });

    void (async () => {
      try {
        unlisten = await onConcertoEvent<StreamEvent>(subject, (event) => {
          // Oneof variants serialize PascalCase (`Session`, `Started`,
          // `Exited`) by prost's serde default — accept both spellings.
          const session = oneofVariant<SessionEventPayload>(
            event.body,
            "Session",
            "session",
          );
          const kind = session?.kind;
          if (!kind) return;
          if (oneofVariant(kind, "Started", "started")) {
            setState({ status: "running", exitCode: null });
          } else {
            const exited = oneofVariant<{ exit_code?: number | null }>(
              kind,
              "Exited",
              "exited",
            );
            if (exited) {
              const code = exited.exit_code ?? null;
              setState({
                status: code === 0 || code === null ? "finished" : "crashed",
                exitCode: code,
              });
            }
          }
        });
        const id = await subscribe(subject);
        if (cancelled) {
          await unsubscribe(id);
          return;
        }
        subscriptionId = id;
      } catch (e) {
        console.error("useSessionEvents failed", subject, e);
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

  return state;
}
