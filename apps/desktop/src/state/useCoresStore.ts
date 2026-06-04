// UI-only active-Core selection (Task 218, `design/15 §3.3`).
//
// Per `design/15 §3.3`, Zustand holds ONLY UI/ephemeral state; everything
// server-canonical lives in React Query. The connected-Core registry (the list
// of paired Cores + which one is active on disk) is server-canonical — it is
// fetched via `src/api/cores.ts` and cached in React Query, NOT duplicated
// here.
//
// What lives here is the renderer's *pending* active-Core selection: the
// `core_id` the user has clicked in the Connect-to-Core picker (Task 219/601)
// before the switch is committed to the registry. It exists so the picker UI
// can reflect the choice immediately (optimistic highlight) while the
// `set_active_core` write + reconnect is in flight. Once committed and
// re-fetched, the React Query `getActiveCore` result is authoritative; this
// slice is cleared.

import { create } from "zustand";

export type CoresStore = {
  /// The `core_id` the user has selected in the picker but not yet committed,
  /// or `null` when there is no pending selection (the committed active Core
  /// from React Query is authoritative). Pure UI ephemera — never persisted,
  /// never the source of truth for which Core is actually connected.
  pendingActiveCoreId: string | null;
  /// Set the pending selection (the picker calls this on click).
  setPendingActiveCore: (coreId: string | null) => void;
  /// Clear the pending selection (called once the switch commits / is cancelled).
  clearPendingActiveCore: () => void;
};

export const useCoresStore = create<CoresStore>((set) => ({
  pendingActiveCoreId: null,
  setPendingActiveCore: (coreId) => set({ pendingActiveCoreId: coreId }),
  clearPendingActiveCore: () => set({ pendingActiveCoreId: null }),
}));
