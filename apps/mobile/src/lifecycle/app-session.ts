// The app-shell glue (Tasks 516 + 518) that ties the lifecycle controller, the
// resumable stream manager, and push registration to the LIVE native transport.
// Kept out of the React tree (a plain factory) so the root layout's hook is a
// one-liner and the wiring stays unit-testable.
//
// On foreground: `openNativeSession()` opens a fresh native DataClient (closing
// it on background via `module.closeSession`), the StreamManager re-subscribes
// the app's subjects from their since_offset, and — once a session is up — the
// device (re)registers its Expo push token with the active Core.
import type { DataClient } from "@concerto/client";

import { openNativeSession } from "../data/app-client";
import { activeCore } from "../pairing/core-store";
import { registerForPush } from "../push/register";
import { NOTIFICATION_EVENTS_SUBJECT } from "@concerto/client";
import { AppLifecycleController, type OpenedSession } from "./app-lifecycle";
import { StreamManager } from "./stream-manager";

/** The app's default streams subjects (Task 518 — what we resume on foreground). */
export const DEFAULT_SUBJECTS = [
  NOTIFICATION_EVENTS_SUBJECT,
  "workspace.events",
  "workarea.events",
] as const;

/** Options for [`createAppSession`]. */
export interface CreateAppSessionOptions {
  /** The shared stream manager (defaults to a fresh one with DEFAULT_SUBJECTS). */
  streams?: StreamManager;
  /** Open a session (defaults to the live `openNativeSession`). Injected in tests. */
  open?: () => Promise<OpenedSession | null>;
  /** Register push after a session opens (defaults to the live registration). */
  registerPush?: (client: DataClient) => Promise<void>;
}

/** The wired app session: the lifecycle controller + its stream manager. */
export interface AppSession {
  controller: AppLifecycleController;
  streams: StreamManager;
}

/**
 * Build the lifecycle controller wired to the live native transport. Foreground
 * opens a session, resubscribes streams from their offsets, and registers push;
 * background closes the session. The push registration is best-effort (a denied
 * permission / no-Core just no-ops) and never blocks the session.
 */
export function createAppSession(opts: CreateAppSessionOptions = {}): AppSession {
  const streams = opts.streams ?? defaultStreams();
  const open = opts.open ?? (() => openNativeSession());
  const registerPush = opts.registerPush ?? defaultRegisterPush;

  const controller = new AppLifecycleController({
    streams,
    openClient: async () => {
      const opened = await open();
      if (!opened) return null;
      // Fire-and-forget push registration once the session is live.
      void registerPush(opened.client).catch(() => {});
      return opened;
    },
  });

  return { controller, streams };
}

function defaultStreams(): StreamManager {
  const mgr = new StreamManager();
  for (const subject of DEFAULT_SUBJECTS) {
    mgr.add({ subject, onEvent: () => {} });
  }
  return mgr;
}

async function defaultRegisterPush(client: DataClient): Promise<void> {
  const core = await activeCore();
  if (!core) return;
  await registerForPush({ client, deviceId: core.deviceIdHex });
}
