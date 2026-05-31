/**
 * Time-to-first-render measurement helpers.
 *
 * The "time-to-render" number the spike reports is the wall-clock span from
 * the moment the user requests a fixture (button press) to the moment the
 * virtualized list has committed its first frame of content. We capture the
 * request timestamp, then read the elapsed time inside the list's
 * `onContentSizeChange` / first `onLoad`-style callback, which fires after the
 * initial visible window has laid out. This intentionally includes the parse +
 * flatten + tokenize cost, because that is what the user waits through.
 */

export function nowMs(): number {
  return typeof globalThis.performance?.now === 'function'
    ? globalThis.performance.now()
    : Date.now();
}

export interface RenderTiming {
  /** Milliseconds from request to first committed content frame. */
  readonly ms: number;
  /** Number of flattened rows produced. */
  readonly rows: number;
  /** Parse + flatten CPU time (subset of `ms`). */
  readonly buildMs: number;
}

export function formatMs(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(2)} s`;
}

/** The V1.0 budget bar (`design/16 §10`, PRD §22.3). */
export const RENDER_BUDGET_MS = 1500;
export const FPS_BUDGET = 60;
