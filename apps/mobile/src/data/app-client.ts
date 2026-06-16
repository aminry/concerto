// The app's data-client wiring (Task 513 seam + Task 510 transport selection).
//
// Two layers:
//   1. `appDataClient()` selects the LIVE transport: the native `ConcertoIroh`
//      `DataClient` when a paired Core + a dev/prod build are present, else a
//      Core-free fallback. This is the seam the screens' live implementation
//      (516+) reads through; it speaks the transport-agnostic `DataClient` from
//      @concerto/client.
//   2. `appWorkspacesClient()` is the screen-shaped facade (Task 513). It still
//      returns the fixture-backed mock so the drill-down renders a representative
//      feed in the app shell pre-live-transport; Task 516 swaps its body to a
//      `DataClient`-backed implementation built from `appDataClient()` — only
//      this factory changes, the screens (which take the seam as a prop) do not.
import { createNativeDataClient } from "./native-data-client";
import { mockWorkspacesClient, type WorkspacesClient } from "./workspaces-client";
import { demoWorkspacesFixture } from "./fixtures";
import { mockChatClient, type ChatClient } from "../chat/chat-client";
import { demoChatFixture } from "../chat/chat-fixtures";
import { activeCore } from "../pairing/core-store";
import {
  getNativeConcertoIroh,
  hasNativeConcertoIroh,
  type ConcertoIrohModule,
} from "../native/ConcertoIroh";
import type { DataClient } from "@concerto/client";

/**
 * Open a native [`DataClient`] against the ACTIVE paired Core (Task 510). Returns
 * `null` when there is no native binding (Expo Go / jest) or no active Core —
 * the caller falls back to the fixture-backed facade. The opened session handle
 * is owned by the returned client's lifetime; the app closes it on background
 * (design/16 §3.12) — wired by a later task.
 *
 * `module` is injectable so a test can pass a `createMockConcertoIroh(...)`
 * instead of the (absent) real native binding.
 */
export async function openNativeDataClient(opts?: {
  module?: ConcertoIrohModule;
}): Promise<DataClient | null> {
  const core = await activeCore();
  if (!core) return null;

  const module = opts?.module ?? (hasNativeConcertoIroh() ? getNativeConcertoIroh() : null);
  if (!module) return null;

  const handle = await module.openSession(
    {
      endpointId: core.blob.endpointId,
      ...(core.blob.relayUrl !== undefined ? { relayUrl: core.blob.relayUrl } : {}),
      directAddrs: core.blob.directAddrs,
      coreNoisePub: core.blob.coreNoisePub,
    },
    core.signedCert,
  );
  return createNativeDataClient(module, handle);
}

let cachedWorkspaces: WorkspacesClient | undefined;

/** The app-wide WorkspacesClient (memoised so screens share one fixture set). */
export function appWorkspacesClient(): WorkspacesClient {
  if (!cachedWorkspaces) {
    cachedWorkspaces = mockWorkspacesClient(demoWorkspacesFixture());
  }
  return cachedWorkspaces;
}

let cachedChat: ChatClient | undefined;

/**
 * The app-wide Concerto ChatClient (Task 512), memoised so the landing tab keeps
 * one transcript across remounts. Fixture-backed in the app shell; Task 512's
 * live path swaps this body to `createLiveChatClient(await openNativeDataClient())`
 * once the `maestro.events` token contract is verified against a live Core
 * (Tier-3) — the screen (which takes the seam as a prop) does not change.
 */
export function appChatClient(): ChatClient {
  if (!cachedChat) {
    cachedChat = mockChatClient(demoChatFixture());
  }
  return cachedChat;
}
