// The Maestro write-tool confirmation-chip PRODUCER (Task 417, design/08 R-2).
//
// The 5 Maestro write tools surface, under strict mode, as the EXISTING
// `AwaitingApproval` confirmation gate on the Maestro singleton session
// (Task 33/406) — NOT a new RPC. This hook is the missing producer: it
// subscribes to that session's `session.events.<sid>` stream, lifts each
// write-tool `AwaitingApproval` frame into `useMaestroStore.pendingConfirmation`
// so `<ConfirmationChip>` renders it, and the chip's Approve/Deny resolves
// through `Sessions.ResolveApproval` (the existing path — no bypass, R-2).
//
// ── Mirrors `SessionRegion.tsx`'s reader EXACTLY ─────────────────────────────
// Same `session.events.<sid>` subject, same dual-spelling `oneofVariant` walk
// (`Session`/`session` → `kind` → `AwaitingApproval`/`awaiting_approval` —
// prost serde defaults to PascalCase, the renderer was written against
// snake_case, so accept both). The ONLY difference vs `SessionRegion` is the
// session id: it comes from `Maestro.GetState.maestro_session_id`, not the
// active terminal session.
//
// Empty session id (Maestro disabled / no live session) ⇒ no subscription,
// no chip — `useEventSubscription("")` is a no-op by contract.

import { useCallback } from "react";

import {
  oneofVariant,
  type AwaitingApproval,
  type StreamEvent,
} from "../../api/sessions";
import { useEventSubscription } from "../../hooks/useEventSubscription";
import { useMaestroStore } from "../../state/useMaestroStore";

/// Read a write-tool `AwaitingApproval` off a `session.events.<sid>` frame, or
/// `null` if the frame is any other kind. Walks the same oneof path
/// `SessionRegion` does: `body.{Session|session}.kind.{AwaitingApproval|
/// awaiting_approval}`. Pure — unit-tested.
export function readAwaitingApproval(
  event: StreamEvent | unknown,
): AwaitingApproval | null {
  const body = (event as { body?: unknown })?.body;
  // Oneof variants serialize PascalCase by prost's serde default;
  // `oneofVariant` accepts both spellings.
  const session = oneofVariant<{ kind?: unknown }>(body, "Session", "session");
  const kind = session?.kind;
  if (!kind) return null;
  // `AwaitingApproval` isn't in the V0.1 `SessionEventPayload.kind` union;
  // read it dynamically (it rides `awaiting_approval = 13`, streams.proto).
  const approval = oneofVariant<AwaitingApproval>(
    kind,
    "AwaitingApproval",
    "awaiting_approval",
  );
  if (!approval || typeof approval.approval_id !== "string") return null;
  return approval;
}

/// Subscribe to the Maestro session's `session.events.<sid>` and lift each
/// write-tool `AwaitingApproval` frame into `pendingConfirmation` so
/// `<ConfirmationChip>` renders it. `maestroSessionId` comes from
/// `Maestro.GetState.maestro_session_id`; an empty string (no live session /
/// Maestro disabled) means no subscription and no chip. Read-only — the gate
/// itself is server-canonical; this only surfaces it. Resolution is the chip's
/// `Sessions.ResolveApproval` call.
export function useMaestroConfirmations(maestroSessionId: string | undefined): void {
  const setPendingConfirmation = useMaestroStore(
    (s) => s.setPendingConfirmation,
  );
  const sid = maestroSessionId ?? "";

  const onFrame = useCallback(
    (event: StreamEvent) => {
      const approval = readAwaitingApproval(event);
      if (approval) {
        setPendingConfirmation({ sessionId: sid, approval });
      }
    },
    [sid, setPendingConfirmation],
  );

  // Empty subject ⇒ `useEventSubscription` skips (no `concerto/` listener).
  useEventSubscription<StreamEvent>(
    sid ? `session.events.${sid}` : "",
    onFrame,
  );
}
