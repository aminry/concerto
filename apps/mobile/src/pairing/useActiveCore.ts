// A tiny hook that reports whether a Core is currently paired/active (Task 512).
// The Concerto landing tab uses it to choose between the unpaired Pair-CTA empty
// state and the live chat surface. It reads the `core-store` (which is backed by
// `expo-secure-store`, jest-mocked Tier-2) and re-checks whenever the screen
// regains focus, so pairing in the modal flow reflects back on the tab.
import { useCallback, useState } from "react";
import { useFocusEffect } from "expo-router";

import { activeCoreId } from "./core-store";

/**
 * `undefined` while the first check is in flight (the caller can render a neutral
 * shell), then `true`/`false`. Re-runs on focus.
 */
export function useHasActiveCore(): boolean | undefined {
  const [hasCore, setHasCore] = useState<boolean | undefined>(undefined);

  useFocusEffect(
    useCallback(() => {
      let cancelled = false;
      void activeCoreId().then((id) => {
        if (!cancelled) setHasCore(!!id);
      });
      return () => {
        cancelled = true;
      };
    }, []),
  );

  return hasCore;
}
