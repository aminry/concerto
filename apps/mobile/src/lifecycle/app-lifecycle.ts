// App background/foreground lifecycle (Task 518; design/16 §3.12 — close the
// native session on background; §6.2 — revalidate + reopen + re-subscribe with a
// since_offset on foreground). On a phone the OS suspends the app when it goes to
// the background: holding an Iroh session open across that is wasteful and racy,
// so we tear it down and rebuild it on return, replaying only what was missed.
//
// The controller is FRAMEWORK-FREE (no React) so it is a pure Tier-2 unit; the
// `useAppLifecycle` hook wires it to RN's `AppState` for the app shell. The
// session-open + session-close collaborators are INJECTED:
//   - `openClient()` opens a fresh native DataClient + returns a `close()` (this
//     is `openNativeDataClient()` + `module.closeSession(handle)` in production),
//   - the `StreamManager` is reopened against the new client and resubscribes
//     every subject from its offset cursor.
import { useEffect, useRef } from "react";
import { AppState, type AppStateStatus } from "react-native";

import type { DataClient } from "@concerto/client";

import type { StreamManager } from "./stream-manager";

/** An opened session: the DataClient + the teardown that closes it. */
export interface OpenedSession {
  client: DataClient;
  /** Close the underlying native session (module.closeSession — design/16 §3.12). */
  close: () => void | Promise<void>;
}

/** Options for [`AppLifecycleController`]. */
export interface AppLifecycleControllerOptions {
  /**
   * Open a fresh session. Returns the DataClient + its `close`, or `null` when
   * there is nothing to open (no paired Core). Called on foreground.
   */
  openClient: () => Promise<OpenedSession | null>;
  /** The stream manager re-opened against each new session. */
  streams: StreamManager;
  /**
   * Optional revalidation run BEFORE reopening on foreground (design/16 §6.2 —
   * e.g. re-check the cert / active Core). Reopen proceeds only if it resolves
   * truthy (default: always reopen).
   */
  revalidate?: () => Promise<boolean>;
}

/**
 * The lifecycle state machine. `foreground()` opens a session + (re)subscribes
 * streams; `background()` closes the session (streams abort, offset cursors are
 * retained). Idempotent: re-entering the same phase is a no-op.
 */
export class AppLifecycleController {
  private opened: OpenedSession | null = null;
  private phase: "foreground" | "background" = "background";

  constructor(private readonly opts: AppLifecycleControllerOptions) {}

  /** Whether a session is currently open. */
  get isOpen(): boolean {
    return this.opened !== null;
  }

  /**
   * Foreground transition (design/16 §6.2): revalidate, open a fresh session, and
   * re-subscribe every stream FROM its since_offset cursor. No-op if already
   * foregrounded with an open session.
   */
  async foreground(): Promise<void> {
    if (this.phase === "foreground" && this.opened) return;
    this.phase = "foreground";

    if (this.opts.revalidate) {
      const ok = await this.opts.revalidate();
      if (!ok) return;
    }
    if (this.phase !== "foreground") return; // raced to background mid-await

    const opened = await this.opts.openClient();
    if (!opened) return;
    if (this.phase !== "foreground") {
      // Backgrounded while opening — close immediately, don't leak the session.
      await opened.close();
      return;
    }
    this.opened = opened;
    // Re-subscribe every subject from its retained offset cursor (since_offset).
    this.opts.streams.open(opened.client);
  }

  /**
   * Background transition (design/16 §3.12): abort streams + close the native
   * session. Offset cursors on the StreamManager are retained for the next
   * foreground. No-op if already backgrounded.
   */
  async background(): Promise<void> {
    if (this.phase === "background" && !this.opened) {
      this.phase = "background";
      return;
    }
    this.phase = "background";
    this.opts.streams.close();
    const opened = this.opened;
    this.opened = null;
    if (opened) await opened.close();
  }
}

/**
 * React hook: drive an [`AppLifecycleController`] from RN `AppState`. `active` ⇒
 * foreground; `background` / `inactive` ⇒ background. The controller + its
 * collaborators are passed in (so the app shell owns construction and tests use
 * the controller directly). Foregrounds once on mount.
 */
export function useAppLifecycle(controller: AppLifecycleController): void {
  const ref = useRef(controller);
  ref.current = controller;

  useEffect(() => {
    void ref.current.foreground();
    const sub = AppState.addEventListener("change", (state: AppStateStatus) => {
      if (state === "active") {
        void ref.current.foreground();
      } else {
        void ref.current.background();
      }
    });
    return () => {
      sub.remove();
      void ref.current.background();
    };
  }, []);
}
