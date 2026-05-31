/**
 * Lightweight on-screen FPS sampler driven by `requestAnimationFrame`.
 *
 * IMPORTANT (read `design/spikes/rn-diff-findings.md` §2): a JS-thread rAF
 * counter measures the *JS frame rate*, which on the New Architecture is a
 * useful proxy but is NOT authoritative for scroll smoothness — RN scrolling
 * runs on the UI thread. The credible 60 fps verdict on a real device must come
 * from Xcode Instruments (Core Animation FPS) or the Android GPU profiler /
 * Perfetto, per `design/16 §10` and Task 103's implementation notes. This
 * counter is here so the operator has an in-app number while profiling and so
 * the simulator run has *some* indicative reading.
 */

export interface FpsSample {
  /** Frames-per-second over the most recent window. */
  readonly fps: number;
  /** Lowest 1-second fps seen since reset (worst-case dips matter most). */
  readonly minFps: number;
}

export class FpsMeter {
  private frames = 0;
  private windowStart = 0;
  private rafId: number | null = null;
  private current: FpsSample = { fps: 0, minFps: Infinity };
  private listeners = new Set<(s: FpsSample) => void>();

  start(): void {
    if (this.rafId !== null) {
      return;
    }
    this.windowStart = now();
    this.frames = 0;
    this.current = { fps: 0, minFps: Infinity };
    const tick = (): void => {
      this.frames++;
      const t = now();
      const elapsed = t - this.windowStart;
      if (elapsed >= 1000) {
        const fps = Math.round((this.frames * 1000) / elapsed);
        const minFps = Math.min(this.current.minFps, fps);
        this.current = { fps, minFps };
        this.frames = 0;
        this.windowStart = t;
        for (const l of this.listeners) {
          l(this.current);
        }
      }
      this.rafId = requestAnimationFrame(tick);
    };
    this.rafId = requestAnimationFrame(tick);
  }

  stop(): void {
    if (this.rafId !== null) {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
  }

  reset(): void {
    this.current = { fps: 0, minFps: Infinity };
    this.windowStart = now();
    this.frames = 0;
  }

  subscribe(listener: (s: FpsSample) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
}

function now(): number {
  return typeof globalThis.performance?.now === 'function'
    ? globalThis.performance.now()
    : Date.now();
}
