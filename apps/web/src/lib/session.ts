//! Web ephemeral-pairing glue (Task 522). Ties the transport-agnostic session
//! machinery in `@concerto/client` to the browser: a stub-phone signer mints an
//! 8h `web_ephemeral` cert, an IndexedDB store persists it, and a small manager
//! holds the live cert so the connect interceptor (set up in `lib/data.ts`) can
//! attach the FROZEN `concerto-device-cert` header on every call.
//!
//! Clear-on-close: by default the session is cleared when the tab is hidden /
//! closed (pagehide + visibilitychange). "Remember browser" opts OUT of the
//! clear — the cert survives in IndexedDB across reloads until it expires (8h).
//!
//! TIER-3: the Core actually TRUSTING this cert needs the real phone signer
//! (Task 511) + the bridge auth middleware to register the ephemeral device.

import {
  createIndexedDbSessionStore,
  createMemorySessionStore,
  createStubPhoneSigner,
  type EphemeralSession,
  isExpired,
  loadValidSession,
  mintEphemeralSession,
  type SessionStore,
  type SignedEphemeralCert,
} from "@concerto/client";

/** A serializable snapshot of the session for the UI status chip. */
export type SessionStatus =
  | { kind: "none" }
  | { kind: "paired"; expiresAt: number; remember: boolean }
  | { kind: "cleared" };

/** Build the right [`SessionStore`] for the environment (IndexedDB, else memory). */
function makeStore(): SessionStore {
  if (typeof globalThis !== "undefined" && (globalThis as { indexedDB?: IDBFactory }).indexedDB) {
    try {
      return createIndexedDbSessionStore();
    } catch {
      // Fall through to memory if IndexedDB construction throws (private mode etc.).
    }
  }
  return createMemorySessionStore();
}

/**
 * The session manager: owns the live ephemeral session + its persistence and
 * exposes the bits the app shell + the connect interceptor need. One per app.
 */
export class SessionManager {
  private store: SessionStore;
  private current: EphemeralSession | null = null;
  private remember = false;
  private listeners = new Set<(s: SessionStatus) => void>();
  private cleanupTabClose: (() => void) | null = null;

  constructor(store: SessionStore = makeStore()) {
    this.store = store;
  }

  /**
   * Read the live cert (for the connect interceptor's per-call `getCert`).
   * Returns null once the cert has expired so an 8h-stale `web_ephemeral` cert
   * is never attached — the next Connect re-mints.
   */
  getCert = (): SignedEphemeralCert | null =>
    this.current && !isExpired(this.current) ? this.current.cert : null;

  /** Subscribe to status changes; returns an unsubscribe. */
  onStatus(fn: (s: SessionStatus) => void): () => void {
    this.listeners.add(fn);
    fn(this.status());
    return () => this.listeners.delete(fn);
  }

  /** Current status snapshot. */
  status(): SessionStatus {
    if (!this.current) return { kind: "none" };
    return { kind: "paired", expiresAt: this.current.expiresAt, remember: this.remember };
  }

  private emit(status: SessionStatus = this.status()): void {
    for (const fn of this.listeners) fn(status);
  }

  /**
   * Ensure a valid session exists: reuse the live one (or a remembered one from
   * IndexedDB) if still valid, otherwise mint a fresh 8h cert via the stub-phone
   * signer. `remember` controls PERSISTENCE: when ON, the cert is written to
   * IndexedDB so it survives a reload; when OFF, the cert lives ONLY in memory
   * (and is wiped from IndexedDB), so a tab close / reload deterministically
   * loses it — this is the clear-on-close opt-out.
   */
  async ensureSession(remember: boolean): Promise<EphemeralSession> {
    this.remember = remember;

    // Reuse a still-valid live session, or a persisted one (only present if a
    // prior session opted into remember).
    const existing = this.current ?? (await loadValidSession(this.store));
    const session = existing ?? (await mintEphemeralSession(await createStubPhoneSigner()));
    this.current = session;
    await this.persist();
    this.armTabClose();
    this.emit();
    return session;
  }

  /** Write or wipe the persisted session per the remember flag. */
  private async persist(): Promise<void> {
    if (this.remember && this.current) {
      await this.store.put({ cert: this.current.cert, remember: true });
    } else {
      // remember OFF → never durably persist (clear-on-close is implicit).
      await this.store.clear();
    }
  }

  /** On boot: adopt a remembered, still-valid session if one is persisted. */
  async restore(): Promise<EphemeralSession | null> {
    const loaded = await loadValidSession(this.store);
    if (loaded) {
      this.current = loaded;
      const stored = await this.store.get();
      this.remember = stored?.remember ?? false;
      this.armTabClose();
      this.emit();
    }
    return loaded;
  }

  /** Clear the session now (in memory + persistence) and report "cleared". */
  async clear(): Promise<void> {
    this.current = null;
    await this.store.clear();
    this.disarmTabClose();
    this.emit({ kind: "cleared" });
  }

  /**
   * Arm the clear-on-close handlers (pagehide + visibilitychange→hidden). A
   * no-op when "remember browser" is on. We CLEAR on hide so a backgrounded tab
   * that the OS may discard does not leak the cert; reconnecting re-mints.
   */
  private armTabClose(): void {
    this.disarmTabClose();
    if (this.remember) return;
    if (typeof window === "undefined") return;

    const onClose = () => {
      // Synchronous best-effort: drop the in-memory cert immediately so no
      // further request can attach it; clear the (memory or IDB) store too.
      this.current = null;
      void this.store.clear();
      // Mirror clear(): tear down the handlers + notify listeners so the status
      // chip flips to "cleared" and the App tears down its live subscription
      // (no cert-less poller keeps running). Without this emit the chip would
      // keep showing "Paired" while getCert() now returns null.
      this.disarmTabClose();
      this.emit({ kind: "cleared" });
    };
    const onVisibility = () => {
      if (document.visibilityState === "hidden") onClose();
    };
    window.addEventListener("pagehide", onClose);
    document.addEventListener("visibilitychange", onVisibility);
    this.cleanupTabClose = () => {
      window.removeEventListener("pagehide", onClose);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }

  private disarmTabClose(): void {
    this.cleanupTabClose?.();
    this.cleanupTabClose = null;
  }
}

/** A single app-wide session manager. */
export const sessionManager = new SessionManager();
