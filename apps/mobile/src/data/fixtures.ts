// Deterministic Workspaces/Workareas/Sessions/PRs fixtures (Task 513) built from
// @concerto/client's REAL generated proto schemas via `create(...)`. Used by the
// app shell (pre-live-transport) and by the unit tests so the RN tree renders a
// stable, type-checked feed without a live Core (PHASE5_PLANNING D11).
//
// The `make*` helpers take a `MessageInitShape` (the loose init object `create`
// accepts — partial, no required `$typeName`) so callers pass plain field
// overrides and get back the strict generated message type.
import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";

import { WorkspaceSchema, type Workspace } from "@concerto/client/gen/concerto/v1/workspaces_pb";
import { WorkareaSchema, type Workarea } from "@concerto/client/gen/concerto/v1/workareas_pb";
import { SessionSchema, type Session } from "@concerto/client/gen/concerto/v1/sessions_pb";
import { PullRequestSchema, type PullRequest } from "@concerto/client/gen/concerto/v1/vcs_pb";

import type { WorkspacesFixture } from "./workspaces-client";

type WorkspaceInit = MessageInitShape<typeof WorkspaceSchema>;
type WorkareaInit = MessageInitShape<typeof WorkareaSchema>;
type SessionInit = MessageInitShape<typeof SessionSchema>;
type PullRequestInit = MessageInitShape<typeof PullRequestSchema>;

const HOUR = 3_600_000;

function ago(ms: number): Date {
  return new Date(Date.now() - ms);
}

export function makeWorkspace(over: WorkspaceInit & { id: string; name: string }): Workspace {
  return create(WorkspaceSchema, {
    slug: over.name.toLowerCase().replace(/\s+/g, "-"),
    createdAt: timestampFromDate(ago(48 * HOUR)),
    ...over,
  });
}

export function makeWorkarea(
  over: WorkareaInit & { id: string; workspaceId: string; composerName: string },
): Workarea {
  return create(WorkareaSchema, {
    branchName: `concerto/${over.composerName}`,
    worktreeRoot: `/Users/dev/concerto/workareas/${over.composerName}`,
    status: "active",
    createdAt: timestampFromDate(ago(6 * HOUR)),
    lastActivityAt: timestampFromDate(ago(HOUR)),
    ...over,
  });
}

export function makeSession(over: SessionInit & { id: string; workareaId: string }): Session {
  return create(SessionSchema, {
    chatId: `chat-${over.id}`,
    agentKind: "claude",
    status: "running",
    startedAt: timestampFromDate(ago(2 * HOUR)),
    ...over,
  });
}

export function makePullRequest(
  over: PullRequestInit & { id: string; workareaId: string; prNumber: bigint; title: string },
): PullRequest {
  return create(PullRequestSchema, {
    provider: "github",
    state: "open",
    baseRef: "main",
    headRef: "concerto/feature",
    url: `https://github.com/acme/web/pull/${over.prNumber}`,
    repositoryFullName: "acme/web",
    ...over,
  });
}

/**
 * A representative, multi-workspace fixture for the app shell + a default in
 * tests. Workspace `ws-web` drills into workarea `wa-aria` with 2 sessions + 1
 * open PR; `ws-core` drills into `wa-bee` with 1 session + 0 PRs.
 */
export function demoWorkspacesFixture(): WorkspacesFixture {
  const workspaces: Workspace[] = [
    makeWorkspace({
      id: "ws-web",
      name: "Web Redesign",
      icon: "🎨",
      description: "Marketing site + app shell refresh",
    }),
    makeWorkspace({
      id: "ws-core",
      name: "Core Runtime",
      icon: "⚙️",
      description: "Supervisor + transport hardening",
    }),
  ];

  const workareas: Record<string, Workarea> = {
    "wa-aria": makeWorkarea({
      id: "wa-aria",
      workspaceId: "ws-web",
      composerName: "aria",
      status: "running",
    }),
    "wa-bee": makeWorkarea({
      id: "wa-bee",
      workspaceId: "ws-core",
      composerName: "bee",
      status: "awaiting",
    }),
  };

  const sessions: Record<string, Session[]> = {
    "wa-aria": [
      makeSession({ id: "se-1", workareaId: "wa-aria", agentKind: "claude", status: "running" }),
      makeSession({ id: "se-2", workareaId: "wa-aria", agentKind: "codex", status: "finished" }),
    ],
    "wa-bee": [
      makeSession({ id: "se-3", workareaId: "wa-bee", agentKind: "claude", status: "awaiting" }),
    ],
  };

  const pullRequests: Record<string, PullRequest[]> = {
    "wa-aria": [
      makePullRequest({
        id: "pr-1",
        workareaId: "wa-aria",
        prNumber: 482n,
        title: "Refresh the landing hero + nav",
        state: "open",
      }),
    ],
    "wa-bee": [],
  };

  return { workspaces, workareas, sessions, pullRequests };
}
