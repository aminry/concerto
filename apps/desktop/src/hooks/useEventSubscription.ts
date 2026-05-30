// React hook that mounts a long-lived subscription on a Tauri
// event channel. The Rust shell forwards each
// `Streams.Subscribe(subject)` frame to the renderer as a
// `concerto/<subject>` event; this hook invokes `callback` per
// frame and tears the subscription down on unmount.
//
// The hook intentionally avoids React Query's `useQuery` here — the
// stream is fire-and-forget. Callers typically pass a callback that
// invalidates a relevant query key so the UI re-fetches.

import { useEffect, useRef } from "react";

import { onConcertoEvent, subscribe, unsubscribe } from "../api/client";

export function useEventSubscription<T>(
  subject: string,
  callback: (payload: T) => void,
): void {
  // Stash the callback in a ref so changes don't tear the
  // subscription down. The Rust forwarder lives for the lifetime of
  // the subscription id, and the listener bridges every frame.
  const callbackRef = useRef(callback);
  callbackRef.current = callback;

  useEffect(() => {
    let subscriptionId: string | null = null;
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    void (async () => {
      try {
        unlisten = await onConcertoEvent<T>(subject, (payload) => {
          callbackRef.current(payload);
        });
        // StrictMode runs cleanup before these awaits resolve and can't
        // see `unlisten` yet — unlisten here if already torn down.
        if (cancelled) {
          unlisten?.();
          return;
        }
        const id = await subscribe(subject);
        if (cancelled) {
          unlisten?.();
          await unsubscribe(id);
          return;
        }
        subscriptionId = id;
      } catch (e) {
        // Surface to the console but don't blow up the render tree;
        // missing subscriptions degrade gracefully to "no live
        // updates", and the manual Refresh button still works.
        console.error("useEventSubscription failed", subject, e);
      }
    })();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
      if (subscriptionId) {
        void unsubscribe(subscriptionId);
      }
    };
  }, [subject]);
}
