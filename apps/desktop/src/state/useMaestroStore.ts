// UI-only Zustand slice for the Maestro chat surface (Task 415).
//
// Per `design/15 §3.3`, Zustand owns NOTHING derived from the Core. The
// digest, the Maestro state (budget counters / enabled), and the transcript
// are SERVER-CANONICAL — they live in React Query keyed off `api/maestro.ts`'s
// bindings (`getDigest` / the `maestro.events` invalidation). This slice holds
// only UI ephemera local to the user's current window session:
//
//   - the composer draft text (so it survives a re-mount while typing);
//   - whether the digest panel is collapsed;
//   - whether the whole chat top bar is collapsed;
//   - the pending write-tool confirmation (the `AwaitingApproval` the user is
//     about to Approve/Deny) — UI selection only; the gate itself is
//     server-canonical on `session.events.<sid>`.
//
// Mirrors the `useUiStore` convention (Task 25/46). Deliberately NOT persisted
// to `localStorage` — these are transient per-session bits, unlike the layout
// state.

import { create } from "zustand";

import type { AwaitingApproval } from "../api/sessions";

/// A pending write-tool confirmation the user is being asked to resolve. Pairs
/// the `AwaitingApproval` frame (server-canonical, from `session.events.<sid>`)
/// with the `sessionId` whose `Sessions.ResolveApproval` resolves it.
export type PendingConfirmation = {
  sessionId: string;
  approval: AwaitingApproval;
};

export type MaestroStore = {
  /// The composer draft (UI ephemera; the sent message is server-canonical).
  composerDraft: string;
  /// Digest panel collapsed (the digest content itself is React-Query-canonical).
  digestCollapsed: boolean;
  /// The whole Concerto-chat top bar collapsed (it is always MOUNTED — this is
  /// just whether its body is shown, design/08 §1).
  chatCollapsed: boolean;
  /// The write-tool confirmation the user is currently being asked to resolve,
  /// or null. UI selection only.
  pendingConfirmation: PendingConfirmation | null;

  setComposerDraft: (text: string) => void;
  setDigestCollapsed: (collapsed: boolean) => void;
  toggleDigestCollapsed: () => void;
  setChatCollapsed: (collapsed: boolean) => void;
  toggleChatCollapsed: () => void;
  setPendingConfirmation: (c: PendingConfirmation | null) => void;
};

export const useMaestroStore = create<MaestroStore>((set) => ({
  composerDraft: "",
  digestCollapsed: false,
  chatCollapsed: false,
  pendingConfirmation: null,

  setComposerDraft: (text) => set({ composerDraft: text }),
  setDigestCollapsed: (collapsed) => set({ digestCollapsed: collapsed }),
  toggleDigestCollapsed: () =>
    set((s) => ({ digestCollapsed: !s.digestCollapsed })),
  setChatCollapsed: (collapsed) => set({ chatCollapsed: collapsed }),
  toggleChatCollapsed: () => set((s) => ({ chatCollapsed: !s.chatCollapsed })),
  setPendingConfirmation: (c) => set({ pendingConfirmation: c }),
}));
