// Voice-dictation seam (Task 515). A small, transport-agnostic speech-recognition
// interface the chat composer's mic button drives. Real on-device STT
// (@react-native-voice/voice or expo-speech-recognition) needs a prebuild /
// custom dev client + a real microphone -> Tier-3. We define the interface + a
// jest mock NOW and wire the UI against it; swapping in the native recognizer is
// then a single factory change with no UI churn (the mic button programs against
// `SpeechRecognizer`, never the native module).
//
// Mic lifecycle the UI renders: idle -> (start) listening -> (onPartial*) ->
// (onFinal) -> idle, or -> error. Permission is checked before `start`.

/** Permission outcome for microphone + speech recognition. */
export type PermissionStatus = "granted" | "denied" | "undetermined";

/** Callbacks for one dictation run, passed to [`SpeechRecognizer.start`]. */
export interface SpeechCallbacks {
  /** Live partial transcript (replaces, not appends) — shown as a preview. */
  onPartial?: (transcript: string) => void;
  /** The settled transcript for this run — the composer fills with this. */
  onFinal?: (transcript: string) => void;
  /** Recognition error (no permission, no speech, engine error, …). */
  onError?: (message: string) => void;
}

/** The small speech-recognition surface the composer programs against. */
export interface SpeechRecognizer {
  /**
   * Whether speech recognition is available at all (a real recognizer returns
   * false in Expo Go / on a simulator without the native module). The UI hides
   * the mic button when false.
   */
  isAvailable(): boolean;
  /**
   * Request (or read) microphone + speech permission. The UI checks this before
   * the first `start` and surfaces the denied path (Settings deep-link is Tier-3).
   */
  requestPermission(): Promise<PermissionStatus>;
  /**
   * Begin listening. `onPartial` may fire repeatedly with the live transcript;
   * `onFinal` fires once with the settled text; `onError` on failure. Calling
   * `start` while already listening is a no-op.
   */
  start(callbacks: SpeechCallbacks): Promise<void>;
  /** Stop listening and settle the current transcript (fires `onFinal`). */
  stop(): Promise<void>;
}

/** Script for [`createMockRecognizer`]: the partials emitted then the final text. */
export interface MockRecognizerScript {
  /** Partial transcripts emitted in order on `start` (each replaces the prior). */
  partials?: string[];
  /** The final transcript emitted on `stop` (or after the last partial on autoFinal). */
  final?: string;
}

/** Options for [`createMockRecognizer`]. */
export interface MockRecognizerOptions {
  /** Permission the mock reports (default "granted"). */
  permission?: PermissionStatus;
  /** Availability the mock reports (default true). */
  available?: boolean;
  /**
   * If true, emit the partials AND the final automatically on `start` (no `stop`
   * needed) — lets a test drive the whole idle->listening->final flow from one
   * action. If false (default), partials fire on `start` and `final` fires on `stop`.
   */
  autoFinal?: boolean;
  /** If set, `start` reports this error via `onError` instead of listening. */
  startError?: string;
}

/**
 * A jest-friendly mock recognizer. With `autoFinal: true` a single `start`
 * deterministically walks partial -> partial -> final; otherwise `start` emits
 * the partials and `stop` settles the final. No timers — callbacks fire
 * synchronously within the awaited call so tests stay timer-free.
 */
export function createMockRecognizer(
  script: MockRecognizerScript = {},
  opts: MockRecognizerOptions = {},
): SpeechRecognizer {
  const permission = opts.permission ?? "granted";
  const available = opts.available ?? true;
  let active: SpeechCallbacks | null = null;

  return {
    isAvailable() {
      return available;
    },
    async requestPermission() {
      return permission;
    },
    async start(callbacks) {
      if (active) return;
      if (opts.startError) {
        callbacks.onError?.(opts.startError);
        return;
      }
      active = callbacks;
      for (const p of script.partials ?? []) {
        callbacks.onPartial?.(p);
      }
      if (opts.autoFinal) {
        active = null;
        callbacks.onFinal?.(script.final ?? script.partials?.at(-1) ?? "");
      }
    },
    async stop() {
      const cb = active;
      active = null;
      cb?.onFinal?.(script.final ?? script.partials?.at(-1) ?? "");
    },
  };
}
