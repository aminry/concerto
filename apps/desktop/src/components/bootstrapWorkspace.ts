// Part 2 — after a workspace is created, auto-create its first workarea
// and first session. The first session's agent is isolated behind
// DEFAULT_FIRST_AGENT so swapping in real availability detection later is
// a one-line change. Only `claude` is implemented server-side today.

import { createWorkarea } from "../api/workareas";
import { createSession } from "../api/sessions";

export const DEFAULT_FIRST_AGENT = "claude";

export type BootstrapResult = {
  workareaId: string;
  sessionId: string;
};

/// Create the first workarea (inheriting workspace/repo cone defaults — no
/// cones passed) then the first session. Throws if either step fails; the
/// caller decides how to surface a partial-bootstrap (the workspace itself
/// is already committed).
export async function bootstrapWorkspace(
  workspaceId: string,
): Promise<BootstrapResult> {
  const workarea = await createWorkarea(workspaceId);
  const session = await createSession({
    workareaId: workarea.id,
    agentKind: DEFAULT_FIRST_AGENT,
  });
  return { workareaId: workarea.id, sessionId: session.id };
}
