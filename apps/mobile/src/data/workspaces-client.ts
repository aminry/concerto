// Mobile workspaces data seam (Task 513). The Workspaces drill-down screens
// (Workspace -> Workarea, NO project tier per D14) read through this narrow,
// transport-agnostic interface so the RN component tree stays decoupled from the
// live transport. The live native transport (the ConcertoIroh `DataClient` from
// @concerto/client) is wired in Task 510/516; until then the screens run against
// `mockWorkspacesClient(...)`, which returns the REAL generated proto types
// (PHASE5_PLANNING D11 — mobile consumes only @concerto/client).
//
// Why a screen-shaped facade instead of using the raw `Workspaces` /
// `Workareas` / `Sessions` / `Vcs` service clients directly: it keeps each
// screen's data contract tiny + mockable in a Tier-2 jest test (no live Core,
// no native module), and it is the single seam Task 510/516 swaps for a real
// `createClient(Service, dc.transport)`-backed implementation.
import type { Workspace } from "@concerto/client/gen/concerto/v1/workspaces_pb";
import type { Workarea } from "@concerto/client/gen/concerto/v1/workareas_pb";
import type { Session } from "@concerto/client/gen/concerto/v1/sessions_pb";
import type { PullRequest } from "@concerto/client/gen/concerto/v1/vcs_pb";

/**
 * The screen-facing data contract for the Workspaces drill-down. Every method is
 * a Promise so the live implementation (Task 510/516) can issue real unary RPCs;
 * the mock resolves synchronously-ish from in-memory fixtures.
 */
export interface WorkspacesClient {
  /** List workspaces (active by default; `includeArchived` opts the rest in). */
  listWorkspaces(opts?: { includeArchived?: boolean }): Promise<Workspace[]>;
  /** List the workareas in a workspace (the drill-down targets, NO project tier per D14). */
  listWorkareas(workspaceId: string, opts?: { includeArchived?: boolean }): Promise<Workarea[]>;
  /** Fetch a single workarea by id (the drill-down target). */
  getWorkarea(id: string): Promise<Workarea>;
  /** List the sessions on a workarea (the "Sessions" segment). */
  listSessions(workareaId: string): Promise<Session[]>;
  /** The workarea's PR set (the "Code & PRs" segment). */
  getWorkareaPrSet(workareaId: string): Promise<PullRequest[]>;
  /**
   * The unified-diff TEXT for a PR (Task 514's Code & PRs diff drill-down).
   * Returns the raw `git diff` patch the RN `DiffView` parses. NOTE: there is no
   * generated proto service for PR diffs yet, so the live transport (Task 516)
   * will route this through whichever Vcs/Files RPC the Core exposes; until then
   * the mock serves a typed fixture. Resolves "" when the PR has no diff.
   */
  getPrDiff(prId: string): Promise<string>;
}

/** In-memory fixture shape backing [`mockWorkspacesClient`]. */
export interface WorkspacesFixture {
  workspaces: Workspace[];
  /** Workareas keyed by their id (the drill-down target). */
  workareas: Record<string, Workarea>;
  /** Sessions keyed by workarea id. */
  sessions: Record<string, Session[]>;
  /** PRs keyed by workarea id. */
  pullRequests: Record<string, PullRequest[]>;
  /** Unified-diff text keyed by PR id (Task 514). Missing key -> "" (no diff). */
  prDiffs?: Record<string, string>;
}

/** Options for [`mockWorkspacesClient`]. */
export interface MockOptions {
  /**
   * If set, every method rejects with this error — drives the screens' error
   * state in tests. The string is surfaced verbatim in the UI.
   */
  failWith?: string;
  /**
   * Artificial delay (ms) before each method resolves — lets a test observe the
   * loading state deterministically. Defaults to 0 (resolve on next microtask).
   */
  delayMs?: number;
}

const EMPTY_FIXTURE: WorkspacesFixture = {
  workspaces: [],
  workareas: {},
  sessions: {},
  pullRequests: {},
  prDiffs: {},
};

function settle<T>(value: () => T, opts: MockOptions): Promise<T> {
  if (opts.failWith) {
    return Promise.reject(new Error(opts.failWith));
  }
  if (opts.delayMs && opts.delayMs > 0) {
    // Resolve/reject after the delay; a thrown `value()` becomes a rejection.
    return new Promise((resolve, reject) =>
      setTimeout(() => {
        try {
          resolve(value());
        } catch (err) {
          reject(err);
        }
      }, opts.delayMs),
    );
  }
  // `Promise.resolve().then(value)` turns a synchronous `throw` (e.g. a missing
  // id) into a rejected promise — matching how the live unary RPC would surface
  // a NOT_FOUND — rather than throwing synchronously out of the call.
  return Promise.resolve().then(value);
}

/**
 * Build a fixture-backed [`WorkspacesClient`] for tests + the pre-live-transport
 * app shell. Returns the REAL generated proto types so the screens are exercised
 * against the same contract the live transport will satisfy in Task 510/516.
 */
export function mockWorkspacesClient(
  fixture: Partial<WorkspacesFixture> = {},
  opts: MockOptions = {},
): WorkspacesClient {
  const data: WorkspacesFixture = { ...EMPTY_FIXTURE, ...fixture };
  return {
    listWorkspaces(o = {}) {
      return settle(
        () =>
          o.includeArchived
            ? data.workspaces
            : data.workspaces.filter((w) => !w.archivedAt),
        opts,
      );
    },
    listWorkareas(workspaceId, o = {}) {
      return settle(
        () =>
          Object.values(data.workareas)
            .filter((wa) => wa.workspaceId === workspaceId)
            .filter((wa) => o.includeArchived || !wa.archivedAt),
        opts,
      );
    },
    getWorkarea(id) {
      return settle(() => {
        const wa = data.workareas[id];
        if (!wa) throw new Error(`workarea ${id} not found`);
        return wa;
      }, opts);
    },
    listSessions(workareaId) {
      return settle(() => data.sessions[workareaId] ?? [], opts);
    },
    getWorkareaPrSet(workareaId) {
      return settle(() => data.pullRequests[workareaId] ?? [], opts);
    },
    getPrDiff(prId) {
      return settle(() => data.prDiffs?.[prId] ?? "", opts);
    },
  };
}
