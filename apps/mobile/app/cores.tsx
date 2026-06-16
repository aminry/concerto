// The Cores picker route (Task 511). `/cores` lists paired Cores (switch /
// remove / pair another) over the secure-store registry. Reached from the
// Concerto tab's "Cores" affordance.
import { useRouter } from "expo-router";

import { CoresScreen } from "../src/pairing/CoresScreen";

export default function CoresRoute() {
  const router = useRouter();
  return (
    <CoresScreen
      onPairAnother={() => router.push("/pair")}
      onClose={() => {
        if (router.canGoBack()) router.back();
        else router.replace("/");
      }}
    />
  );
}
