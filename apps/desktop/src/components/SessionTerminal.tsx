// xterm.js panel for a single session.
//
// Owns the terminal lifecycle (mount → subscribe → unsubscribe →
// dispose) and bridges the two directions:
//
//   - Inbound: bytes from `session.io.<sid>` → `terminal.write(...)`
//     (via the `useSessionIO` hook).
//   - Outbound: terminal keystrokes → `Sessions.SendMessage` with the
//     raw bytes (xterm hands us a UTF-8 string on `onData`; we encode
//     with `TextEncoder` so the wire bytes are unambiguous).
//
// Task 26 pre-decision (1): we skip the `react-xtermjs` wrapper and
// write the React glue inline — the xterm `Terminal` API is stable
// and the wrapper buys us little for the surface we actually use.
// Pre-decision (3): WebGL renderer is best-effort; canvas fallback
// is automatic.

import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";

import { sendMessage } from "../api/sessions";
import { useSessionIO } from "../hooks/useSessionIO";
import { useTheme } from "../hooks/useTheme";
import { THEME_COLORS, TERMINAL_ANSI } from "../theme/tokens";

export type SessionTerminalProps = {
  sessionId: string;
  /// True after a Stop has been issued — the terminal stays mounted
  /// but stops accepting keystrokes so the user can scroll the
  /// existing buffer.
  disabled?: boolean;
};

/// xterm init options pinned by `tasks/26 §Public interface this
/// task locks`. Don't change without the task seal review.
const XTERM_OPTIONS = {
  cols: 120,
  rows: 30,
  allowProposedApi: true,
  fontFamily:
    "'JetBrains Mono', 'SF Mono', Menlo, Consolas, 'Liberation Mono', monospace",
  fontSize: 13,
  convertEol: true,
} as const;

/// Build an xterm ITheme from the app's effective theme. The viewport
/// background is rendered transparent via CSS (see `.xterm-viewport`),
/// so the panel's `bg-surface` shows through; we still set `background`
/// here so xterm's own fills (e.g. cell backgrounds) match.
function xtermTheme(effective: "light" | "dark") {
  const c = THEME_COLORS[effective];
  return {
    background: c.surface,
    foreground: c.foreground,
    cursor: c.accent,
    cursorAccent: c.surface,
    selectionBackground: effective === "dark" ? "#33415580" : "#c7d2fe80",
    ...TERMINAL_ANSI[effective],
  };
}

export function SessionTerminal({
  sessionId,
  disabled = false,
}: SessionTerminalProps): JSX.Element {
  const { effective } = useTheme();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const sendQueueRef = useRef<Uint8Array[]>([]);
  const sendingRef = useRef(false);
  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;
  // Latest effective theme, read at construction time without adding
  // `effective` to the mount effect's deps (that would tear down and
  // recreate the terminal on every theme toggle — losing scrollback).
  const effectiveRef = useRef(effective);
  effectiveRef.current = effective;

  // Mount the xterm instance once on session change.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const terminal = new Terminal({
      ...XTERM_OPTIONS,
      theme: xtermTheme(effectiveRef.current),
    });
    const fitAddon = new FitAddon();
    const webLinksAddon = new WebLinksAddon();
    terminal.loadAddon(fitAddon);
    terminal.loadAddon(webLinksAddon);
    try {
      const webglAddon = new WebglAddon();
      terminal.loadAddon(webglAddon);
    } catch (e) {
      // WebGL fails on some virtualised displays. xterm falls back to
      // its canvas renderer automatically; the catch keeps the panel
      // alive instead of throwing on mount.
      console.warn("WebGL addon unavailable, using canvas renderer", e);
    }
    terminal.open(container);
    try {
      fitAddon.fit();
    } catch (e) {
      // FitAddon throws if the container has zero size at mount.
      // The ResizeObserver below catches the first real layout pass.
      console.warn("initial fit failed; will retry on resize", e);
    }

    terminalRef.current = terminal;
    fitAddonRef.current = fitAddon;

    // Encode keystrokes once per onData callback and enqueue. The
    // queue serialises Sessions.SendMessage calls so a fast-typer
    // doesn't interleave bytes across in-flight RPCs.
    const encoder = new TextEncoder();
    const onDataDisposable = terminal.onData((data) => {
      if (disabledRef.current) return;
      const bytes = encoder.encode(data);
      sendQueueRef.current.push(bytes);
      void drainSendQueue(sessionId, sendQueueRef, sendingRef);
    });

    // ResizeObserver re-fits the terminal on container resize.
    // Tauri's window resize and React layout changes both flow through
    // here. We do NOT propagate dimensions to the agent in V0.1 —
    // the PTY size is fixed by the host. Phase 3 may add resize
    // forwarding.
    const resizeObserver = new ResizeObserver(() => {
      try {
        fitAddon.fit();
      } catch (e) {
        console.warn("fit failed on resize", e);
      }
    });
    resizeObserver.observe(container);

    return () => {
      onDataDisposable.dispose();
      resizeObserver.disconnect();
      terminal.dispose();
      terminalRef.current = null;
      fitAddonRef.current = null;
      sendQueueRef.current = [];
      sendingRef.current = false;
    };
  }, [sessionId]);

  // Re-apply the theme live when `effective` changes — without
  // recreating the terminal, so scrollback and the PTY stay intact.
  useEffect(() => {
    const term = terminalRef.current;
    if (term) term.options.theme = xtermTheme(effective);
  }, [effective]);

  // Inbound: write every chunk to the terminal. xterm.js accepts
  // either a string or a Uint8Array; the latter avoids the JS-side
  // UTF-8 decode and keeps control sequences intact.
  useSessionIO(sessionId, (bytes) => {
    const terminal = terminalRef.current;
    if (!terminal) return;
    terminal.write(bytes);
  });

  return (
    <div
      ref={containerRef}
      className="flex-1 min-h-0 bg-surface rounded border border-border"
      // The data attribute keeps debug overlays cheap.
      data-session-id={sessionId}
      data-disabled={disabled ? "true" : "false"}
    />
  );
}

async function drainSendQueue(
  sessionId: string,
  queueRef: React.MutableRefObject<Uint8Array[]>,
  sendingRef: React.MutableRefObject<boolean>,
): Promise<void> {
  if (sendingRef.current) return;
  sendingRef.current = true;
  try {
    while (queueRef.current.length > 0) {
      const next = queueRef.current.shift()!;
      try {
        await sendMessage(sessionId, next);
      } catch (e) {
        // Surface but don't crash the terminal; the user can stop
        // and restart the session if input is dropping.
        console.error("Sessions.SendMessage failed", e);
      }
    }
  } finally {
    sendingRef.current = false;
  }
}
