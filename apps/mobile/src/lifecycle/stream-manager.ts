// A resumable subscription manager (Task 518; design/10 §3.3 — `since_offset`
// replay, design/16 §6.2 — foreground re-subscribe). The app subscribes to a set
// of streams subjects; on background the native session is closed (the streams
// die with it); on foreground the session is reopened and EACH subject is
// re-subscribed FROM the highest offset it durably observed (`since_offset`), so
// the Core replays only what was missed and then transitions to live.
//
// It talks to the typed `Streams` client over `DataClient.transport` directly
// (rather than the convenience `DataClient.subscribe`, which hardcodes
// `{ subject }` with no offset) so it can thread `since_offset`. The DataClient
// is supplied per-(re)open by the lifecycle controller; the manager only tracks
// the offset cursors + active aborts. Pure Tier-2 — a mock DataClient drives it.
import { createClient } from "@connectrpc/connect";
import type { DataClient } from "@concerto/client";
import type { Event } from "@concerto/client/gen/concerto/v1/streams_pb";
import { Streams } from "@concerto/client/gen/concerto/v1/streams_pb";

/** A subject the app cares about + how to handle its frames. */
export interface SubjectSubscription {
  /** The streams subject, e.g. "notification.events". */
  subject: string;
  /** Fires per `Event` frame. */
  onEvent: (ev: Event) => void;
  /** Fires if the underlying stream errors. */
  onError?: (err: unknown) => void;
}

/** One live subscription's bookkeeping. */
interface LiveSub extends SubjectSubscription {
  /** Aborts the in-flight stream. */
  abort: AbortController;
  /** Highest offset observed on this subject (the re-subscribe cursor). */
  lastOffset: bigint;
}

/**
 * Tracks the app's stream subscriptions across session open/close, replaying from
 * the last observed offset on each reopen. NOT bound to one DataClient instance —
 * `open(dc)` (re)binds it to a freshly opened session.
 */
export class StreamManager {
  private subs = new Map<string, LiveSub>();
  private dc: DataClient | null = null;

  /** Register a subject (idempotent on `subject`). If a session is already open,
   *  it is subscribed immediately (from offset 0). */
  add(sub: SubjectSubscription): void {
    if (this.subs.has(sub.subject)) return;
    const live: LiveSub = {
      ...sub,
      abort: new AbortController(),
      lastOffset: 0n,
    };
    this.subs.set(sub.subject, live);
    if (this.dc) this.start(live, this.dc);
  }

  /** The offset cursor for a subject (highest observed). 0n if unknown. */
  offsetFor(subject: string): bigint {
    return this.subs.get(subject)?.lastOffset ?? 0n;
  }

  /** Subjects currently registered. */
  subjects(): string[] {
    return [...this.subs.keys()];
  }

  /**
   * (Re)open against a DataClient and (re)subscribe every registered subject.
   * Each subject resumes FROM its last observed offset (`since_offset`), so a
   * reopen after a background gap replays only the missed frames. Aborts any
   * stale streams from a prior session first.
   */
  open(dc: DataClient): void {
    this.dc = dc;
    for (const live of this.subs.values()) {
      live.abort.abort();
      live.abort = new AbortController();
      this.start(live, dc);
    }
  }

  /** Close: abort every live stream and forget the DataClient. Offset cursors are
   *  RETAINED so the next `open` resumes from them. */
  close(): void {
    for (const live of this.subs.values()) {
      live.abort.abort();
    }
    this.dc = null;
  }

  private start(live: LiveSub, dc: DataClient): void {
    const streams = createClient(Streams, dc.transport);
    const ac = live.abort;
    void (async () => {
      try {
        const req =
          live.lastOffset > 0n
            ? { subject: live.subject, sinceOffset: live.lastOffset }
            : { subject: live.subject };
        for await (const ev of streams.subscribe(req, { signal: ac.signal })) {
          if (ev.offset > live.lastOffset) live.lastOffset = ev.offset;
          live.onEvent(ev);
        }
      } catch (err) {
        if (!ac.signal.aborted) live.onError?.(err);
      }
    })();
  }
}
