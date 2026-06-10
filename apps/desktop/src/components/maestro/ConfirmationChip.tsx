// The write-tool confirmation chip (Task 415, design/08 R-2 / D4).
//
// The 5 Maestro write tools (`route_prompt_to_session`, `fanout_to_sessions`,
// `create_workspace`, `create_workarea`, `set_workarea_paused`) + `propose_chip`
// surface, under strict mode, as the EXISTING `AwaitingApproval` confirmation
// gate (Task 33/43) — NOT a new RPC. Approve/Deny resolve through
// `Sessions.ResolveApproval` verbatim (the same path `SessionRegion` uses). No
// bypass (R-2): every user-visible side effect confirms.
//
// `urgent` (red styling) + `destructive_label` come straight off the frozen
// `AwaitingApproval` wire shape (streams.proto:286, `urgent=5`/`destructive_
// label=6`). The gate itself is server-canonical on `session.events.<sid>`; the
// `useMaestroStore` slice only holds the UI SELECTION of which gate is showing.

import { useCallback, useState } from "react";

import {
  ApprovalDecision,
  resolveApproval,
  type ApprovalDecisionValue,
  type AwaitingApproval,
} from "../../api/sessions";
import { formatError } from "../../api/errors";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";

export type ConfirmationChipProps = {
  /// The session whose `Sessions.ResolveApproval` resolves this gate.
  sessionId: string;
  approval: AwaitingApproval;
  /// Called after a decision resolves (to clear the pending selection).
  onResolved?: () => void;
};

export function ConfirmationChip({
  sessionId,
  approval,
  onResolved,
}: ConfirmationChipProps): JSX.Element {
  const [resolving, setResolving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const resolve = useCallback(
    async (decision: ApprovalDecisionValue) => {
      if (resolving) return;
      setResolving(true);
      setError(null);
      try {
        await resolveApproval(sessionId, approval.approval_id, decision);
        onResolved?.();
      } catch (e) {
        setError(formatError(e));
      } finally {
        setResolving(false);
      }
    },
    [resolving, sessionId, approval.approval_id, onResolved],
  );

  const urgent = !!approval.urgent;

  return (
    <div
      data-testid="confirmation-chip"
      data-urgent={urgent ? "true" : "false"}
      className={`rounded-md border px-3 py-2 ${
        urgent
          ? "border-err/40 bg-err/10"
          : "border-border bg-surface-2"
      }`}
    >
      <div className="flex items-center gap-2">
        <span className="text-xs font-semibold uppercase tracking-wide text-muted">
          Confirm
        </span>
        <Badge variant="neutral">{approval.tool}</Badge>
        {urgent && approval.destructive_label && (
          <Badge
            variant="neutral"
            className="border-err/40 text-err"
            data-testid="destructive-label"
          >
            {approval.destructive_label}
          </Badge>
        )}
      </div>
      <p
        className={`pt-1 text-sm ${urgent ? "text-err" : "text-foreground"}`}
      >
        {approval.summary}
      </p>
      <div className="flex gap-2 pt-2">
        <Button
          variant={urgent ? "danger" : "primary"}
          size="sm"
          onClick={() => void resolve(ApprovalDecision.APPROVE)}
          disabled={resolving}
        >
          {resolving ? "…" : "Approve"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void resolve(ApprovalDecision.DENY)}
          disabled={resolving}
        >
          Deny
        </Button>
      </div>
      {error && (
        <p className="pt-1 text-xs text-err whitespace-normal break-words">
          {error}
        </p>
      )}
    </div>
  );
}
