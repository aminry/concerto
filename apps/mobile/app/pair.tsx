// The "Pair a Core" route (Task 511). `/pair` mounts the QR scanner over the
// REAL native pairing module (Tier-3); on success it returns to where the user
// came from. Lives outside `(tabs)` so it presents over the tab bar. The entry
// points are the Concerto tab's "Pair a Core" affordance and the Cores picker.
import { useRouter } from "expo-router";

import { PairScreen } from "../src/pairing/PairScreen";

export default function PairRoute() {
  const router = useRouter();
  return (
    <PairScreen
      onPaired={() => {
        // Back to the previous screen; the active Core is now the new one.
        if (router.canGoBack()) router.back();
        else router.replace("/");
      }}
      onCancel={() => {
        if (router.canGoBack()) router.back();
        else router.replace("/");
      }}
    />
  );
}
