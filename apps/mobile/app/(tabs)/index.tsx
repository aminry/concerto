// Concerto tab — the default landing (design/16 §3.4). The live chat surface
// lands in Task 512+; for the scaffold it is a placeholder PLUS the Task 511
// "Pair a Core" / "Manage Cores" entry points so a fresh install can pair before
// the chat exists.
import { View } from "react-native";
import { useRouter } from "expo-router";

import { Placeholder } from "../../src/Placeholder";
import { PairEntry } from "../../src/pairing/PairEntry";

export default function ConcertoTab() {
  const router = useRouter();
  return (
    <View style={{ flex: 1 }}>
      <Placeholder
        title="Concerto"
        subtitle="Your chat with Concerto lands here. Pair a Core to get started."
      />
      <View style={{ position: "absolute", bottom: 48, left: 0, right: 0 }}>
        <PairEntry
          onPair={() => router.push("/pair")}
          onManageCores={() => router.push("/cores")}
        />
      </View>
    </View>
  );
}
