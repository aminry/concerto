// The app's speech-recognizer selection (Task 515).
//
// Real on-device STT (@react-native-voice/voice or expo-speech-recognition) needs
// a prebuild / custom dev client + a real microphone, so it is TIER-3. Until that
// native module is added + verified on a device, `appRecognizer()` returns an
// UNAVAILABLE recognizer: `isAvailable()` is false, so the composer simply hides
// the mic button (graceful degradation in Expo Go / jest / the simulator). The
// interface + UI are fully wired now (Task 515) — landing real STT is then a
// single swap of this factory's body, no UI change.
import type { SpeechRecognizer } from "./speech-recognizer";

/** A recognizer that reports itself unavailable (the pre-native-STT state). */
export function unavailableRecognizer(): SpeechRecognizer {
  return {
    isAvailable: () => false,
    requestPermission: async () => "undetermined",
    start: async () => {},
    stop: async () => {},
  };
}

/**
 * The app-wide [`SpeechRecognizer`]. TIER-3: returns the unavailable recognizer
 * until a native STT module (e.g. `expo-speech-recognition`) is added behind a
 * prebuild and verified on a device; the composer hides the mic until then.
 */
export function appRecognizer(): SpeechRecognizer {
  return unavailableRecognizer();
}
