// Concerto tab — the default landing (design/16 §3.4 / D14: "Concerto" is the
// user-facing name for the Maestro chat). Mounts the `ChatScreen` over the app's
// `ChatClient` seam (Task 512), chooses the unpaired Pair-CTA empty state when no
// Core is paired (reusing the Task 511 pairing entry), and forwards the app's
// (Tier-3) voice recognizer to the composer's mic (Task 515).
import { useRouter } from "expo-router";

import { ChatScreen } from "../../src/chat/ChatScreen";
import { appChatClient } from "../../src/data/app-client";
import { useHasActiveCore } from "../../src/pairing/useActiveCore";
import { appRecognizer } from "../../src/voice/app-recognizer";

export default function ConcertoTab() {
  const router = useRouter();
  const hasCore = useHasActiveCore();
  return (
    <ChatScreen
      client={appChatClient()}
      hasCore={hasCore ?? false}
      onPair={() => router.push("/pair")}
      onManageCores={() => router.push("/cores")}
      recognizer={appRecognizer()}
    />
  );
}
